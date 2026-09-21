use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;
use crate::packet::score::rewrite_reset_score;

pub struct Protocol1_20_3To1_20_2;

impl Protocol for Protocol1_20_3To1_20_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_20_3,
            to: JavaMinecraftVersion::V_1_20_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::RESET_SCORE, reset_score);
    }
}

/// 1.20.2 clears a score with `SET_SCORE` action 1.
fn reset_score(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let out = rewrite_reset_score(wrapper.remaining())
        .ok_or(TranslateError::Unsupported("reset score"))?;
    wrapper.replace_remaining(out);
    wrapper.set_packet(&clientbound::play::SET_SCORE);
    Ok(())
}
