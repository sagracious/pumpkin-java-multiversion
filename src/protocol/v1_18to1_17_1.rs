use crate::api::types::{BOOL, I8, STRING, U8, VAR_INT};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::chunk_legacy;
use crate::packet::mappings::clientbound;
use crate::packet::mappings::serverbound;
use pumpkin_util::version::JavaMinecraftVersion;

pub struct Protocol1_18To1_17_1;

impl Protocol for Protocol1_18To1_17_1 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_18,
            to: JavaMinecraftVersion::V_1_17_1,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.serverbound_layout(&serverbound::play::CLIENT_INFORMATION, client_information);
        reg.clientbound_layout(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, chunk);
    }
}

/// 1.17 and 1.17.1 send "text filtering disabled", 1.18 flipped the field to
/// "enabled". Core reads no server listing flag below 1.18, so none is added.
fn client_information(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&STRING)?;
    wrapper.passthrough(&I8)?;
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&BOOL)?;
    wrapper.passthrough(&U8)?;
    wrapper.passthrough(&VAR_INT)?;
    let disabled = wrapper.read(&BOOL)?;
    wrapper.write(&BOOL, &!disabled)?;
    Ok(())
}

/// 1.17.1 names its sections with a bit mask, takes the biomes as one array
/// for the whole column and reads light from its own packet.
fn chunk(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let out = chunk_legacy::to_v1_17(
        wrapper.remaining(),
        connection.entity_tracker.min_y,
        &ctx.mappings.blockstates,
        ctx.layout,
    )
    .ok_or(TranslateError::Unsupported("chunk"))?;
    wrapper.replace_remaining(out);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::translate_serverbound;
    use pumpkin_protocol::ServerPacket;
    use pumpkin_protocol::java::server::play::SClientInformationPlay;

    const PLAY: u8 = 5;

    /// `md('1.17.1').protocol.play.toServer.packet_settings`: locale, view
    /// distance, chat flags, chat colours, skin parts, main hand and
    /// `disableTextFiltering`. 1.18 renames the last one and adds
    /// `enableServerListing`.
    fn sent_by_client(version: JavaMinecraftVersion, filtering_off: bool) -> Vec<u8> {
        let mut bytes = vec![5, b'e', b'n', b'_', b'u', b's', 8, 0, 1, 0x7f, 1];
        if version >= JavaMinecraftVersion::V_1_17 {
            bytes.push(u8::from(filtering_off));
        }
        if version >= JavaMinecraftVersion::V_1_18 {
            bytes.push(1);
        }
        bytes
    }

    fn read_by_core(version: JavaMinecraftVersion, payload: &[u8]) -> bool {
        let out = translate_serverbound(
            0,
            version,
            PLAY,
            serverbound::play::CLIENT_INFORMATION.to_id(version),
            payload,
        )
        .unwrap();
        let mut read: &[u8] = &out.payload;
        let packet = SClientInformationPlay::read(&mut read, &version).unwrap();
        assert!(read.is_empty(), "{version}");
        packet.text_filtering
    }

    #[test]
    fn the_1_17_flag_reaches_core_the_right_way_round() {
        for version in [JavaMinecraftVersion::V_1_17, JavaMinecraftVersion::V_1_17_1] {
            assert!(!read_by_core(version, &sent_by_client(version, true)));
            assert!(read_by_core(version, &sent_by_client(version, false)));
        }
    }

    #[test]
    fn the_1_18_flag_is_left_alone() {
        let version = JavaMinecraftVersion::V_1_18;
        assert!(read_by_core(version, &sent_by_client(version, true)));
    }

    #[test]
    fn a_1_16_payload_has_no_flag_to_turn() {
        let version = JavaMinecraftVersion::V_1_16_2;
        assert!(!read_by_core(version, &sent_by_client(version, false)));
    }
}
