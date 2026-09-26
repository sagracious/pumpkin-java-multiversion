use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::VAR_INT;
use crate::api::{
    Ctx, MappingData, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection,
};
use crate::packet::mappings::{clientbound, serverbound};

pub struct Protocol1_21_9To1_21_7;

impl Protocol for Protocol1_21_9To1_21_7 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_9,
            to: JavaMinecraftVersion::V_1_21_7,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.serverbound(
            &serverbound::play::DEBUG_SAMPLE_SUBSCRIPTION,
            debug_subscription_request,
        );
        reg.cancel_clientbound(&clientbound::play::DEBUG_BLOCK_VALUE);
        reg.cancel_clientbound(&clientbound::play::DEBUG_CHUNK_VALUE);
        reg.cancel_clientbound(&clientbound::play::DEBUG_ENTITY_VALUE);
        reg.cancel_clientbound(&clientbound::play::DEBUG_EVENT);
        reg.cancel_clientbound(&clientbound::play::GAME_TEST_HIGHLIGHT_POS);
    }
}

fn debug_subscription_request(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    // Pumpkin's 26.3 reader still accepts the sample-type VarInt that older
    // 1.21.6/1.21.7 clients send; Via's registry-list target is proxy-specific.
    wrapper.passthrough(&VAR_INT)?;
    wrapper.set_packet(&serverbound::play::DEBUG_SUBSCRIPTION_REQUEST);
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::ser::NetworkWriteExt;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    #[test]
    fn debug_sample_request_is_renamed_for_pumpkins_single_varint_reader() {
        let protocol = Protocol1_21_9To1_21_7;
        let packet = &serverbound::play::DEBUG_SAMPLE_SUBSCRIPTION;
        let target = &serverbound::play::DEBUG_SUBSCRIPTION_REQUEST;
        let mut registry = Registry::default();
        protocol.register(&mut registry);
        let handler = registry
            .serverbound_handler(packet)
            .expect("older debug request must be translated")
            .handler;

        for (key, version, sample_type) in [(1, V::V_1_21_6, 0), (2, V::V_1_21_7, 3)] {
            assert_ne!(packet.to_id(version), -1, "{version}");
            let mut payload = Vec::new();
            VAR_INT.write(&mut payload, &VarInt(sample_type)).unwrap();
            let mut wrapper = PacketWrapper::new(packet, &payload);
            let mut connection = UserConnection::new(key, version);
            let ctx = Ctx {
                step: protocol.step(),
                mappings: MappingData::get().step(V::V_1_21_9),
                layout: version,
            };

            handler(&mut wrapper, &mut connection, &ctx).unwrap();
            let translated = wrapper.finish().unwrap().unwrap();
            assert!(std::ptr::eq(translated.packet, target));
            assert_eq!(translated.payload, payload, "{version}");
        }
    }

    #[test]
    fn debug_packets_without_an_older_equivalent_are_cancelled() {
        let protocol = Protocol1_21_9To1_21_7;
        let packets = [
            &clientbound::play::DEBUG_BLOCK_VALUE,
            &clientbound::play::DEBUG_CHUNK_VALUE,
            &clientbound::play::DEBUG_ENTITY_VALUE,
            &clientbound::play::DEBUG_EVENT,
            &clientbound::play::GAME_TEST_HIGHLIGHT_POS,
        ];
        let mut registry = Registry::default();
        protocol.register(&mut registry);
        for (key, packet) in packets.into_iter().enumerate() {
            let handler = registry
                .clientbound_handler(packet)
                .expect("unsupported debug packet must be cancelled")
                .handler;
            let mut wrapper = PacketWrapper::new(packet, &[]);
            let mut connection = UserConnection::new(100 + key as u64, V::V_1_21_7);
            let ctx = Ctx {
                step: protocol.step(),
                mappings: MappingData::get().step(V::V_1_21_9),
                layout: V::V_1_21_9,
            };
            handler(&mut wrapper, &mut connection, &ctx).unwrap();
            assert!(wrapper.finish().unwrap().is_none(), "{}", packet.v26_3);
        }
    }
}
