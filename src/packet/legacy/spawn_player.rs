use std::io::Write;

use pumpkin_protocol::{
    ClientPacket, MultiVersionJavaPacket, ServerPacket, VarInt,
    ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError},
};
use pumpkin_util::{math::vector3::Vector3, version::JavaMinecraftVersion};
use uuid::Uuid;

use crate::packet::mappings::clientbound::play::SPAWN_PLAYER;

/// `SPAWN_PLAYER` (`named_entity_spawn`), used up to 1.20.1; 1.20.2 folded players into `ADD_ENTITY`.
#[derive(Clone, Debug, PartialEq)]
pub struct CSpawnPlayer {
    pub entity_id: VarInt,
    pub player_uuid: Uuid,
    pub position: Vector3<f64>,
    pub yaw: u8,
    pub pitch: u8,
}

impl MultiVersionJavaPacket for CSpawnPlayer {
    fn to_id(version: JavaMinecraftVersion) -> i32 {
        SPAWN_PLAYER.to_id(version)
    }
}

impl CSpawnPlayer {
    #[must_use]
    pub const fn new(
        entity_id: VarInt,
        player_uuid: Uuid,
        position: Vector3<f64>,
        yaw: u8,
        pitch: u8,
    ) -> Self {
        Self {
            entity_id,
            player_uuid,
            position,
            yaw,
            pitch,
        }
    }
}

impl ClientPacket for CSpawnPlayer {
    fn write_packet_data(
        &self,
        mut write: impl Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        write.write_var_int(&self.entity_id)?;
        write.write_uuid(&self.player_uuid)?;
        write.write_f64_be(self.position.x)?;
        write.write_f64_be(self.position.y)?;
        write.write_f64_be(self.position.z)?;
        write.write_u8(self.yaw)?;
        write.write_u8(self.pitch)
    }
}

impl<'a> ServerPacket<'a> for CSpawnPlayer {
    fn read(read: &mut &'a [u8], _version: &JavaMinecraftVersion) -> Result<Self, ReadingError> {
        let entity_id = read.get_var_int()?;
        let player_uuid = read.get_uuid()?;
        let position = Vector3::new(read.get_f64_be()?, read.get_f64_be()?, read.get_f64_be()?);
        let yaw = read.get_u8()?;
        let pitch = read.get_u8()?;
        Ok(Self {
            entity_id,
            player_uuid,
            position,
            yaw,
            pitch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CSpawnPlayer;
    use pumpkin_protocol::{ClientPacket, MultiVersionJavaPacket, VarInt};
    use pumpkin_util::{math::vector3::Vector3, version::JavaMinecraftVersion};

    const TIER_4: &[JavaMinecraftVersion] = &[
        JavaMinecraftVersion::V_1_16_2,
        JavaMinecraftVersion::V_1_16_4,
        JavaMinecraftVersion::V_1_17_1,
        JavaMinecraftVersion::V_1_18_2,
        JavaMinecraftVersion::V_1_19,
        JavaMinecraftVersion::V_1_19_1,
        JavaMinecraftVersion::V_1_19_3,
        JavaMinecraftVersion::V_1_19_4,
        JavaMinecraftVersion::V_1_20,
    ];

    #[test]
    fn layout_matches_minecraft_data_for_the_whole_tier() {
        let packet = CSpawnPlayer::new(
            VarInt(11),
            uuid::Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10),
            Vector3::new(0.5, 80.0, -0.5),
            64,
            0,
        );
        for version in TIER_4 {
            let mut out = Vec::new();
            packet
                .write_packet_data(&mut out, version)
                .unwrap_or_else(|e| panic!("{version}: {e}"));
            assert_eq!(out.len(), 43, "{version}: unexpected payload length");
            assert_eq!(out[0], 11, "{version}: entity id");
            assert_eq!(
                f64::from_be_bytes(out[17..25].try_into().unwrap()),
                0.5,
                "{version}: x"
            );
            assert_eq!(out[41], 64, "{version}: yaw comes before pitch");
            assert_eq!(out[42], 0, "{version}: pitch");
        }
    }

    #[test]
    fn the_packet_ends_at_1_20_1() {
        for version in TIER_4 {
            assert_ne!(
                CSpawnPlayer::to_id(*version),
                -1,
                "{version} should still have SPAWN_PLAYER"
            );
        }
        for version in [
            JavaMinecraftVersion::V_1_20_2,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_26_3,
        ] {
            assert_eq!(CSpawnPlayer::to_id(version), -1, "{version} still has it?");
        }
    }
}
