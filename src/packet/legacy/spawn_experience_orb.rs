use std::io::Write;

use pumpkin_protocol::{
    ClientPacket, MultiVersionJavaPacket, ServerPacket, VarInt,
    ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError},
};
use pumpkin_util::{math::vector3::Vector3, version::JavaMinecraftVersion};

use crate::packet::mappings::clientbound::play::SPAWN_EXPERIENCE_ORB;

/// `SPAWN_EXPERIENCE_ORB`, used up to and including 1.21.4; 1.21.5 folded orbs into `ADD_ENTITY`.
/// The count always reads 0 since core never puts it in `ADD_ENTITY`'s data, so the client draws the smallest orb.
#[derive(Clone, Debug, PartialEq)]
pub struct CSpawnExperienceOrb {
    pub entity_id: VarInt,
    pub position: Vector3<f64>,
    pub count: i16,
}

impl MultiVersionJavaPacket for CSpawnExperienceOrb {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        SPAWN_EXPERIENCE_ORB.to_id(version)
    }
}

impl CSpawnExperienceOrb {
    #[must_use]
    pub const fn new(entity_id: VarInt, position: Vector3<f64>, count: i16) -> Self {
        Self {
            entity_id,
            position,
            count,
        }
    }
}

impl ClientPacket for CSpawnExperienceOrb {
    fn write_packet_data(
        &self,
        mut write: impl Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write.write_var_int(&self.entity_id)?;
        write.write_f64_be(self.position.x)?;
        write.write_f64_be(self.position.y)?;
        write.write_f64_be(self.position.z)?;
        write.write_i16_be(self.count)
    }
}

impl<'a> ServerPacket<'a> for CSpawnExperienceOrb {
    fn read(read: &mut &'a [u8], _version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let entity_id = read.get_var_int()?;
        let position = Vector3::new(read.get_f64_be()?, read.get_f64_be()?, read.get_f64_be()?);
        let count = read.get_i16_be()?;
        Ok(Self {
            entity_id,
            position,
            count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CSpawnExperienceOrb;
    use pumpkin_protocol::{ClientPacket, MultiVersionJavaPacket, VarInt};
    use pumpkin_util::{math::vector3::Vector3, version::JavaMinecraftVersion};

    const WRAPPED: &[JavaMinecraftVersion] = &[
        JavaMinecraftVersion::V_1_16_2,
        JavaMinecraftVersion::V_1_17_1,
        JavaMinecraftVersion::V_1_18_2,
        JavaMinecraftVersion::V_1_19_4,
        JavaMinecraftVersion::V_1_20,
        JavaMinecraftVersion::V_1_20_2,
        JavaMinecraftVersion::V_1_20_5,
        JavaMinecraftVersion::V_1_21,
        JavaMinecraftVersion::V_1_21_4,
    ];

    #[test]
    fn layout_matches_minecraft_data_from_1_16_2_to_1_21_4() {
        let packet = CSpawnExperienceOrb::new(VarInt(12), Vector3::new(1.0, 80.0, 2.0), 7);
        for version in WRAPPED {
            let mut out = Vec::new();
            packet
                .write_packet_data(&mut out, version)
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            assert_eq!(out.len(), 27, "{version}: unexpected payload length");
            assert_eq!(out[0], 12, "{version}: entity id");
            assert_eq!(
                f64::from_be_bytes(out[9..17].try_into().unwrap()),
                80.0,
                "{version}: y"
            );
            assert_eq!(
                i16::from_be_bytes(out[25..27].try_into().unwrap()),
                7,
                "{version}: count"
            );
        }
    }

    #[test]
    fn the_packet_ends_at_1_21_4() {
        for version in WRAPPED {
            assert_ne!(
                CSpawnExperienceOrb::to_id(*version),
                -1,
                "{version} should still have SPAWN_EXPERIENCE_ORB"
            );
        }
        for version in [JavaMinecraftVersion::V_1_21_5, JavaMinecraftVersion::V_26_3] {
            assert_eq!(
                CSpawnExperienceOrb::to_id(version),
                -1,
                "{version} still has it?"
            );
        }
    }
}
