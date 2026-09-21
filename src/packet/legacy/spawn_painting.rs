use crate::packet::mappings::clientbound::play::SPAWN_PAINTING;
use crate::remap::painting_variant_id_remap::remap_motive_id_for_version;
use pumpkin_protocol::{
    ClientPacket, MultiVersionJavaPacket, ServerPacket, VarInt,
    ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError},
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::version::JavaMinecraftVersion;
use uuid::Uuid;

/// Variant lives in entity metadata, not the spawn packet, so it's unknown at this point;
/// index 0 (`kebab`) is the identity remap down to 1.7.6.
pub const DEFAULT_VARIANT: VarInt = VarInt(0);

/// Converts the 3D facing index 26.3 uses to the 2D value `SPAWN_PAINTING` wants; `None` for vertical/out-of-range.
#[must_use]
pub const fn direction_2d_from_3d_index(index: i32) -> Option<u8> {
    match index {
        2 => Some(2), // north
        3 => Some(0), // south
        4 => Some(1), // west
        5 => Some(3), // east
        _ => None,
    }
}

/// Spawns a painting entity for versions <= 1.18.2; from 1.19 paintings go through `CSpawnEntity`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CSpawnPainting {
    pub entity_id: VarInt,
    pub uuid: Uuid,
    /// Only used below 1.13, where the motive travels as a bounded string.
    pub title: String,
    /// 26.3 numbering; remapped to the client's numbering on write.
    pub variant: VarInt,
    pub location: BlockPos,
    /// `Direction.get2DDataValue()`; build with [`direction_2d_from_3d_index`], not a yaw angle.
    pub direction: u8,
}

impl MultiVersionJavaPacket for CSpawnPainting {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        SPAWN_PAINTING.to_id(version)
    }
}

impl CSpawnPainting {
    #[must_use]
    pub const fn new(
        entity_id: VarInt,
        uuid: Uuid,
        title: String,
        variant: VarInt,
        location: BlockPos,
        direction: u8,
    ) -> Self {
        Self {
            entity_id,
            uuid,
            title,
            variant,
            location,
            direction,
        }
    }
}

impl ClientPacket for CSpawnPainting {
    fn write_packet_data(
        &self,
        mut write: impl std::io::Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write.write_var_int(&self.entity_id)?;
        if *version >= JavaMinecraftVersion::V_1_9 {
            write.write_uuid(&self.uuid)?;
        }
        if *version >= JavaMinecraftVersion::V_1_13 {
            let remapped_variant = remap_motive_id_for_version(self.variant.0 as u32, *version);
            write.write_var_int(&VarInt(remapped_variant as i32))?;
        } else {
            write.write_string_bounded(&self.title, 13)?;
        }

        if *version >= JavaMinecraftVersion::V_1_8 {
            write.write_block_pos(&self.location, version)?;
            write.write_u8(self.direction)?;
        } else {
            write.write_i32_be(self.location.0.x)?;
            write.write_i32_be(self.location.0.y)?;
            write.write_i32_be(self.location.0.z)?;
            write.write_i32_be(self.direction as i32)?;
        }

        Ok(())
    }
}

impl<'a> ServerPacket<'a> for CSpawnPainting {
    fn read(bytebuf: &mut &'a [u8], version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let entity_id = bytebuf.get_var_int()?;
        let uuid = if *version >= JavaMinecraftVersion::V_1_9 {
            bytebuf.get_uuid()?
        } else {
            Uuid::nil()
        };

        let (title, variant) = if *version >= JavaMinecraftVersion::V_1_13 {
            let variant = bytebuf.get_var_int()?;
            (String::new(), variant)
        } else {
            let title = bytebuf.get_str()?.to_string();
            (title, VarInt(0))
        };

        let (location, direction) = if *version >= JavaMinecraftVersion::V_1_8 {
            let loc = bytebuf.get_block_pos(version)?;
            let dir = bytebuf.get_u8()?;
            (loc, dir)
        } else {
            let x = bytebuf.get_i32_be()?;
            let y = bytebuf.get_i32_be()?;
            let z = bytebuf.get_i32_be()?;
            let dir = bytebuf.get_i32_be()? as u8;
            (BlockPos::new(x, y, z), dir)
        };

        Ok(Self {
            entity_id,
            uuid,
            title,
            variant,
            location,
            direction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CSpawnPainting, DEFAULT_VARIANT, direction_2d_from_3d_index};
    use pumpkin_protocol::{ClientPacket, VarInt};
    use pumpkin_util::{math::position::BlockPos, version::JavaMinecraftVersion};

    const WRAPPED: &[JavaMinecraftVersion] = &[
        JavaMinecraftVersion::V_1_16_2,
        JavaMinecraftVersion::V_1_16_4,
        JavaMinecraftVersion::V_1_17_1,
        JavaMinecraftVersion::V_1_18_2,
    ];

    #[test]
    fn the_four_horizontal_facings_map_onto_their_2d_values() {
        assert_eq!(direction_2d_from_3d_index(3), Some(0)); // south
        assert_eq!(direction_2d_from_3d_index(4), Some(1)); // west
        assert_eq!(direction_2d_from_3d_index(2), Some(2)); // north
        assert_eq!(direction_2d_from_3d_index(5), Some(3)); // east
    }

    #[test]
    fn vertical_and_out_of_range_facings_are_dropped() {
        for index in [-1, 0, 1, 6, 7, 1000] {
            assert_eq!(
                direction_2d_from_3d_index(index),
                None,
                "index {index} should not produce a facing"
            );
        }
    }

    #[test]
    fn layout_matches_minecraft_data_for_1_16_2_to_1_18_2() {
        let painting = CSpawnPainting::new(
            VarInt(9),
            uuid::Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10),
            String::new(),
            DEFAULT_VARIANT,
            BlockPos::new(1, 70, -3),
            direction_2d_from_3d_index(3).expect("south"),
        );
        for version in WRAPPED {
            let mut out = Vec::new();
            painting
                .write_packet_data(&mut out, version)
                .unwrap_or_else(|e| panic!("{version}: {e}"));

            assert_eq!(out.len(), 27, "{version}: unexpected payload length");
            assert_eq!(out[0], 9, "{version}: entity id");
            assert_eq!(out[17], 0, "{version}: motive is the default variant");
            assert_eq!(out[26], 0, "{version}: direction is the 2D south value");
        }
    }
}
