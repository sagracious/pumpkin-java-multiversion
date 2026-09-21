use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};

pub struct Protocol26_3To26_2;

impl Protocol for Protocol26_3To26_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_26_3,
            to: JavaMinecraftVersion::V_26_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // Recipe displays carry item ids inside a codec nothing rewrites yet.
        reg.cancel_clientbound(&clientbound::play::RECIPE_BOOK_ADD);
        reg.serverbound(&serverbound::play::SWING, punch);
    }
}

/// 26.3 renamed `swing` to `punch`; the payload is unchanged.
fn punch(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.set_packet(&serverbound::play::PUNCH);
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
