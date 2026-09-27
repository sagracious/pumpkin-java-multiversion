use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{BOOL, F32, F64, U8, VAR_INT};
use crate::api::{PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;

pub struct Protocol1_19_4To1_19_3;

impl Protocol for Protocol1_19_4To1_19_3 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_19_4,
            to: JavaMinecraftVersion::V_1_19_3,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound_layout(&clientbound::play::PLAYER_POSITION, player_position);
    }
}

/// 1.19.3 and older retain the dismount-vehicle boolean.
fn player_position(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &crate::api::Ctx,
) -> Result<(), TranslateError> {
    for _ in 0..3 {
        wrapper.passthrough(&F64)?;
    }
    wrapper.passthrough(&F32)?;
    wrapper.passthrough(&F32)?;
    wrapper.passthrough(&U8)?;
    wrapper.passthrough(&VAR_INT)?;
    wrapper.write(&BOOL, &false)?;
    Ok(())
}
