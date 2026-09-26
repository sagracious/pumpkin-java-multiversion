use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Ctx, MappingData, PacketWrapper, Protocol, Registry, Step, UserConnection};
use crate::packet::mappings::serverbound;

pub struct Protocol1_21_4To1_21_2;

impl Protocol for Protocol1_21_4To1_21_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_4,
            to: JavaMinecraftVersion::V_1_21_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // 1.21.4+ removed PICK_ITEM and has no serverbound equivalent.
        reg.cancel_serverbound(&serverbound::play::PICK_ITEM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    #[test]
    fn removed_pick_item_is_explicitly_cancelled() {
        let protocol = Protocol1_21_4To1_21_2;
        let packet = &serverbound::play::PICK_ITEM;
        assert_ne!(packet.to_id(V::V_1_21_2), -1);
        assert_eq!(packet.v26_3, -1);

        let mut registry = Registry::default();
        protocol.register(&mut registry);
        let handler = registry
            .serverbound_handler(packet)
            .expect("removed packet must have a cancellation handler")
            .handler;
        for (key, version) in [(1, V::V_1_21_2), (2, V::V_1_16_2)] {
            assert_ne!(packet.to_id(version), -1, "{version}");
            let mut wrapper = PacketWrapper::new(packet, &[]);
            let mut connection = UserConnection::new(key, version);
            let context = Ctx {
                step: protocol.step(),
                mappings: MappingData::get().step(V::V_1_21_4),
                layout: version,
            };

            handler(&mut wrapper, &mut connection, &context).unwrap();
            assert!(wrapper.finish().unwrap().is_none(), "{version}");
        }
    }
}
