use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{BOOL, I16T, ItemT, VAR_INT};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::serverbound;

#[derive(Default)]
struct PlayerHandStorage {
    current_hand: i16,
}

pub struct Protocol1_16_4To1_16_3;

impl Protocol for Protocol1_16_4To1_16_3 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_16_4,
            to: JavaMinecraftVersion::V_1_16_3,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.serverbound(&serverbound::play::SET_CARRIED_ITEM, set_carried_item);
        reg.serverbound(&serverbound::play::EDIT_BOOK, edit_book);
    }
}

/// Keep the selected hotbar slot for the edit-book packet, whose 1.16.3
/// meaning is a hand selector rather than the selected inventory slot.
fn set_carried_item(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let current_hand = wrapper.passthrough(&I16T)?;
    connection.put(PlayerHandStorage { current_hand });
    Ok(())
}

/// 1.16.3 sends the edited stack, a signing flag, and a hand selector. The
/// next protocol step expects the selected hotbar slot, or slot 40 for offhand.
fn edit_book(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&ItemT::for_version(ctx.step.to))?;
    wrapper.passthrough(&BOOL)?;
    let hand = wrapper.read(&VAR_INT)?.0;
    let slot = if hand == 1 {
        40
    } else {
        i32::from(
            connection
                .get::<PlayerHandStorage>()
                .map_or(0, |storage| storage.current_hand),
        )
    };
    wrapper.write(&VAR_INT, &slot)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(payload: &[u8], current_hand: Option<i16>) -> Vec<u8> {
        let mut connection = UserConnection::new(1, JavaMinecraftVersion::V_1_16_3);
        if let Some(current_hand) = current_hand {
            connection.put(PlayerHandStorage { current_hand });
        }
        let step = Protocol1_16_4To1_16_3.step();
        let ctx = Ctx {
            step,
            mappings: crate::api::MappingData::get().step(step.from),
            layout: step.to,
        };
        let mut wrapper = PacketWrapper::new(&serverbound::play::EDIT_BOOK, payload);
        edit_book(&mut wrapper, &mut connection, &ctx).unwrap();
        wrapper.finish().unwrap().unwrap().payload
    }

    #[test]
    fn edit_book_maps_main_hand_to_selected_hotbar_slot() {
        assert_eq!(run(&[0, 0, 0], Some(4)), [0, 0, 4]);
    }

    #[test]
    fn edit_book_maps_offhand_to_slot_forty() {
        assert_eq!(run(&[0, 0, 1], Some(4)), [0, 0, 40]);
    }

    #[test]
    fn edit_book_defaults_to_hotbar_slot_zero_without_a_selection_packet() {
        assert_eq!(run(&[0, 0, 0], None), [0, 0, 0]);
    }
}
