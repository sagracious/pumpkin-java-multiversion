use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{I8, I16T, U8};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::serverbound;

pub struct Protocol1_17_1To1_17;

impl Protocol for Protocol1_17_1To1_17 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_17_1,
            to: JavaMinecraftVersion::V_1_17,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.serverbound_layout(&serverbound::play::CONTAINER_CLICK, container_click);
    }
}

/// 1.17.1 put a state id between the window id and the slot. The client has no
/// state to send, so -1 goes in and the server resyncs the container. Core
/// reads the field as a Short below 1.17.1, which is the width written here.
fn container_click(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let window = wrapper.read(&U8)?;
    let slot = wrapper.read(&I16T)?;
    let button = wrapper.read(&I8)?;
    let mode = wrapper.read(&U8)?;
    if mode > 6 {
        return Err(TranslateError::Unsupported("slot action"));
    }
    wrapper.write(&U8, &window)?;
    wrapper.write(&I16T, &-1)?;
    wrapper.write(&I16T, &slot)?;
    wrapper.write(&I8, &button)?;
    wrapper.write(&U8, &mode)?;
    wrapper.passthrough_all();
    Ok(())
}
