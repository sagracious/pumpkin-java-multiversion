use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::{
    mappings::{clientbound, serverbound},
    recipe_book,
};

mod block;
mod chat;
mod entity;
mod registry;

pub struct Protocol26_3To26_2;

impl Protocol for Protocol26_3To26_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_26_3,
            to: JavaMinecraftVersion::V_26_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // Recipe displays carry nested slot displays, template items and holder sets.
        reg.clientbound_layout(
            &clientbound::play::RECIPE_BOOK_ADD,
            recipe_book::rewrite_recipe_book_add,
        );
        reg.clientbound_layout(
            &clientbound::play::UPDATE_RECIPES,
            recipe_book::rewrite_update_recipes,
        );
        reg.clientbound_layout(
            &clientbound::play::PLACE_GHOST_RECIPE,
            recipe_book::rewrite_place_ghost_recipe,
        );
        reg.serverbound(&serverbound::play::SWING, punch);

        entity::register(reg);
        block::register(reg);
        chat::register(reg);
        registry::register(reg);
    }
}

/// 26.3 renamed `swing` to `punch`; offhand swings have no legacy equivalent.
fn punch(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let hand = wrapper.read(&crate::api::types::VAR_INT)?.0;
    wrapper.set_packet(&serverbound::play::PUNCH);
    if hand == 1 {
        // ViaBackwards drops offhand swings: 26.3's punch packet causes a
        // different action, and there is no equivalent 26.2 packet.
        wrapper.cancel();
        return Ok(());
    }
    wrapper.write(
        &crate::api::types::VAR_INT,
        &pumpkin_protocol::codec::var_int::VarInt(hand),
    )?;
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{PacketWrapper, Protocol, UserConnection};
    use crate::packet::mappings::serverbound::play::{PUNCH, SWING};
    use pumpkin_util::version::JavaMinecraftVersion;

    #[test]
    fn swing_and_punch_are_the_two_halves_of_one_rename() {
        assert_eq!(SWING.v26_3, -1);
        assert_ne!(PUNCH.v26_3, -1);
        for version in [
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_1_20_2,
            JavaMinecraftVersion::V_1_16_2,
        ] {
            assert_ne!(SWING.to_id(version), -1, "{version}");
            assert_eq!(PUNCH.to_id(version), -1, "{version}");
        }
    }

    #[test]
    fn offhand_swing_is_cancelled_but_main_hand_becomes_punch() {
        let version = JavaMinecraftVersion::V_26_2;
        let ctx = Ctx {
            step: Protocol26_3To26_2.step(),
            mappings: crate::api::MappingData::get().step(JavaMinecraftVersion::V_26_3),
            layout: version,
        };

        let mut offhand = PacketWrapper::new(&SWING, &[1]);
        punch(&mut offhand, &mut UserConnection::new(1, version), &ctx).unwrap();
        assert!(offhand.finish().unwrap().is_none());

        let mut main_hand = PacketWrapper::new(&SWING, &[0]);
        punch(&mut main_hand, &mut UserConnection::new(2, version), &ctx).unwrap();
        let translated = main_hand.finish().unwrap().unwrap();
        assert_eq!(
            std::ptr::from_ref(translated.packet),
            std::ptr::from_ref(&PUNCH)
        );
        assert_eq!(translated.payload, [0]);
    }
}
