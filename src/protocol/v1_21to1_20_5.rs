use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::chat;
use crate::api::types::OptionalT;
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;

pub struct Protocol1_21To1_20_5;

impl Protocol for Protocol1_21To1_20_5 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21,
            to: JavaMinecraftVersion::V_1_20_5,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound_layout(&clientbound::play::PLAYER_CHAT, player_chat);
        reg.clientbound_layout(&clientbound::play::DISGUISED_CHAT, disguised_chat);
    }
}

/// 1.20.5 reads the chat type as a plain registry id.
fn player_chat(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    chat::signed_head(wrapper, true)?;
    wrapper.passthrough(&OptionalT(chat::text(connection)))?;
    chat::filter_mask(wrapper, true)?;
    chat::chat_type_to_id(wrapper)?;
    wrapper.passthrough_all();
    Ok(())
}

fn disguised_chat(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&chat::text(connection))?;
    chat::chat_type_to_id(wrapper)?;
    wrapper.passthrough_all();
    Ok(())
}
