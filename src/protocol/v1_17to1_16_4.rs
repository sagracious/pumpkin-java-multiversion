use crate::api::rewriter::chat;
use crate::api::types::{BOOL, F64, I8, I16T, I32, U8, VAR_INT, VAR_LONG, WireType};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::chunk_legacy;
use crate::packet::mappings::{clientbound, serverbound};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion;

pub struct Protocol1_17To1_16_4;

impl Protocol for Protocol1_17To1_16_4 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_17,
            to: JavaMinecraftVersion::V_1_16_4,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.serverbound_layout(&serverbound::play::CONTAINER_CLICK, container_click);
        reg.clientbound_layout(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, chunk);
        reg.clientbound_layout(&clientbound::play::SET_TITLE_TEXT, title_text);
        reg.clientbound_layout(&clientbound::play::SET_SUBTITLE_TEXT, subtitle_text);
        reg.clientbound_layout(&clientbound::play::SET_ACTION_BAR_TEXT, action_bar_text);
        reg.clientbound_layout(&clientbound::play::SET_TITLES_ANIMATION, titles_animation);
        reg.clientbound_layout(&clientbound::play::CLEAR_TITLES, clear_titles);
        reg.clientbound_layout(&clientbound::play::SET_BORDER_SIZE, border_size);
        reg.clientbound_layout(&clientbound::play::SET_BORDER_LERP_SIZE, border_lerp_size);
        reg.clientbound_layout(&clientbound::play::SET_BORDER_CENTER, border_center);
        reg.clientbound_layout(&clientbound::play::INITIALIZE_BORDER, border_initialize);
        reg.clientbound_layout(&clientbound::play::SET_BORDER_WARNING_DELAY, border_delay);
        reg.clientbound_layout(
            &clientbound::play::SET_BORDER_WARNING_DISTANCE,
            border_distance,
        );
        reg.clientbound_layout(&clientbound::play::PLAYER_COMBAT_ENTER, combat_enter);
        reg.clientbound_layout(&clientbound::play::PLAYER_COMBAT_END, combat_end);
        reg.clientbound_layout(&clientbound::play::PLAYER_COMBAT_KILL, combat_kill);
    }
}

/// 1.17 sends changed slots instead of the clicked stack; nothing here can predict those,
/// so the list goes out empty and the clicked stack becomes the carried one.
fn container_click(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let window = wrapper.read(&U8)?;
    let slot = wrapper.read(&I16T)?;
    let button = wrapper.read(&I8)?;
    let action = wrapper.read(&I16T)?;
    let mode = wrapper.read(&U8)?;
    if mode > 6 {
        return Err(TranslateError::Unsupported("slot action"));
    }
    wrapper.write(&U8, &window)?;
    wrapper.write(&I16T, &slot)?;
    wrapper.write(&I8, &button)?;
    wrapper.write(&U8, &mode)?;
    wrapper.write(&VAR_INT, &VarInt(0))?;
    wrapper.passthrough_all();

    wrapper.send_reply(
        &clientbound::play::WINDOW_CONFIRMATION,
        confirmation(window as i8, action)?,
    );
    Ok(())
}

fn confirmation(window: i8, action: i16) -> Result<Vec<u8>, TranslateError> {
    let mut payload = Vec::with_capacity(4);
    I8.write(&mut payload, &window)?;
    I16T.write(&mut payload, &action)?;
    BOOL.write(&mut payload, &true)?;
    Ok(payload)
}

/// 1.16 reads a "full chunk" flag and a varint mask where 1.17 reads a bit set.
fn chunk(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let out = chunk_legacy::to_v1_16(wrapper.remaining(), &ctx.mappings.blockstates)
        .ok_or(TranslateError::Unsupported("chunk"))?;
    wrapper.replace_remaining(out);
    Ok(())
}

/// 1.16.4 keeps titles, the world border and combat in one packet each, with
/// the split form's meaning in a leading action.
fn merge(
    wrapper: &mut PacketWrapper,
    packet: &'static crate::packet::mappings::PacketId,
    action: i32,
) -> Result<(), TranslateError> {
    wrapper.write(&VAR_INT, &VarInt(action))?;
    wrapper.set_packet(packet);
    Ok(())
}

fn title(
    wrapper: &mut PacketWrapper,
    connection: &UserConnection,
    action: i32,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::TITLE, action)?;
    wrapper.passthrough(&chat::text(connection))?;
    Ok(())
}

fn title_text(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    title(wrapper, connection, 0)
}

fn subtitle_text(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    title(wrapper, connection, 1)
}

fn action_bar_text(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    title(wrapper, connection, 2)
}

fn titles_animation(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::TITLE, 3)?;
    for _ in 0..3 {
        wrapper.passthrough(&I32)?;
    }
    Ok(())
}

/// Action 4 hides the title, action 5 also puts the timings back.
fn clear_titles(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let reset = wrapper.read(&BOOL)?;
    merge(
        wrapper,
        &clientbound::play::TITLE,
        if reset { 5 } else { 4 },
    )
}

fn border_size(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::WORLD_BORDER, 0)?;
    wrapper.passthrough(&F64)?;
    Ok(())
}

fn border_lerp_size(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::WORLD_BORDER, 1)?;
    wrapper.passthrough(&F64)?;
    wrapper.passthrough(&F64)?;
    wrapper.passthrough(&VAR_LONG)?;
    Ok(())
}

fn border_center(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::WORLD_BORDER, 2)?;
    wrapper.passthrough(&F64)?;
    wrapper.passthrough(&F64)?;
    Ok(())
}

fn border_initialize(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::WORLD_BORDER, 3)?;
    for _ in 0..4 {
        wrapper.passthrough(&F64)?;
    }
    wrapper.passthrough(&VAR_LONG)?;
    for _ in 0..3 {
        wrapper.passthrough(&VAR_INT)?;
    }
    Ok(())
}

fn border_delay(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::WORLD_BORDER, 4)?;
    wrapper.passthrough(&VAR_INT)?;
    Ok(())
}

fn border_distance(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::WORLD_BORDER, 5)?;
    wrapper.passthrough(&VAR_INT)?;
    Ok(())
}

fn combat_enter(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::COMBAT_EVENT, 0)
}

fn combat_end(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::COMBAT_EVENT, 1)?;
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&I32)?;
    Ok(())
}

fn combat_kill(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    merge(wrapper, &clientbound::play::COMBAT_EVENT, 2)?;
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&I32)?;
    wrapper.passthrough(&chat::text(connection))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::translate_serverbound;
    use pumpkin_protocol::ServerPacket;
    use pumpkin_protocol::java::server::play::SClickSlot;

    const PLAY: u8 = 5;

    /// `md('1.16.2').protocol.play.toServer.packet_window_click`: window id,
    /// slot, mouse button, action number, mode and the clicked stack.
    fn sent_by_client() -> Vec<u8> {
        vec![1, 0, 36, 0, 0, 7, 0, 0]
    }

    #[test]
    fn a_1_16_click_reaches_core_with_a_state_id_and_no_changed_slots() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let out = translate_serverbound(
            0,
            version,
            PLAY,
            serverbound::play::CONTAINER_CLICK.to_id(version),
            &sent_by_client(),
        )
        .unwrap();

        let mut read: &[u8] = &out.payload;
        let packet = SClickSlot::read(&mut read, &version).unwrap();
        assert!(read.is_empty());
        assert_eq!(packet.sync_id, VarInt(1));
        assert_eq!(packet.revision, VarInt(-1), "forces a resync");
        assert_eq!(packet.slot, 36);
        assert_eq!(packet.button, 0);
        assert_eq!(packet.length_of_array, VarInt(0));
        assert!(packet.carried_item.0.is_none());
    }

    #[test]
    fn the_click_is_answered_with_the_confirmation_the_client_waits_for() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let out = translate_serverbound(
            0,
            version,
            PLAY,
            serverbound::play::CONTAINER_CLICK.to_id(version),
            &sent_by_client(),
        )
        .unwrap();

        let (packet, payload) = out.replies.first().expect("a confirmation");
        assert_eq!(
            packet.to_id(version),
            clientbound::play::WINDOW_CONFIRMATION.to_id(version)
        );
        assert_eq!(payload, &[1, 0, 7, 1], "window, action number, accepted");
    }

    #[test]
    fn a_1_17_click_is_not_answered() {
        let version = JavaMinecraftVersion::V_1_17;
        let out = translate_serverbound(
            0,
            version,
            PLAY,
            serverbound::play::CONTAINER_CLICK.to_id(version),
            &[1, 0, 36, 0, 0, 0, 0],
        )
        .unwrap();
        assert!(out.replies.is_empty());
    }
}

#[cfg(test)]
mod player_tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::packet::mappings::PacketId;
    use crate::pipeline::translate_clientbound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::var_long::VarLong;
    use pumpkin_protocol::java::client::play::{
        CClearTitle, CCombatDeath, CCombatEnd, CCombatEnter, CInitializeWorldBorder,
        CSetBorderCenter, CSetBorderLerpSize, CSetBorderSize, CSetBorderWarningDelay,
        CSetBorderWarningDistance, CSubtitle, CTitleAnimation, CTitleText,
    };
    use pumpkin_util::text::TextComponent;
    const PLAY: u8 = 5;
    const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_1_16_2;

    fn merged(key: u64, packet: &'static PacketId, body: &impl ClientPacket) -> Vec<u8> {
        let mut payload = Vec::new();
        body.write_packet_data(&mut payload, &VERSION).unwrap();
        let out = translate_clientbound(key, VERSION, PLAY, packet.v26_3, &payload).unwrap();
        remove_connection(key);
        out.payload
    }

    fn merged_into(
        key: u64,
        packet: &'static PacketId,
        target: &'static PacketId,
        body: &impl ClientPacket,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        body.write_packet_data(&mut payload, &VERSION).unwrap();
        let out = translate_clientbound(key, VERSION, PLAY, packet.v26_3, &payload).unwrap();
        assert_eq!(out.packet.to_id(VERSION), target.to_id(VERSION));
        remove_connection(key);
        out.payload
    }

    /// minecraft-data 1.16.2 `packet_title`: an action varint, then the text
    /// for actions 0 to 2 and three ints for action 3.
    #[test]
    fn the_five_title_packets_become_one() {
        let title = TextComponent::text("hi");
        let mut text = Vec::new();
        CTitleText::new(&title)
            .write_packet_data(&mut text, &VERSION)
            .unwrap();

        let out = merged_into(
            80,
            &clientbound::play::SET_TITLE_TEXT,
            &clientbound::play::TITLE,
            &CTitleText::new(&title),
        );
        assert_eq!(out[0], 0);
        assert_eq!(&out[1..], &text[..]);

        assert_eq!(
            merged(
                81,
                &clientbound::play::SET_SUBTITLE_TEXT,
                &CSubtitle::new(&title)
            )[0],
            1
        );
        assert_eq!(
            merged(
                82,
                &clientbound::play::SET_TITLES_ANIMATION,
                &CTitleAnimation::new(1, 2, 3)
            ),
            [3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3]
        );
        assert_eq!(
            merged(
                83,
                &clientbound::play::CLEAR_TITLES,
                &CClearTitle::new(false)
            ),
            [4]
        );
        assert_eq!(
            merged(
                84,
                &clientbound::play::CLEAR_TITLES,
                &CClearTitle::new(true)
            ),
            [5]
        );
    }

    /// minecraft-data 1.16.2 `packet_world_border`: an action varint, then the
    /// fields of the 1.17 packet the action stands for, the lerp time still a
    /// varlong.
    #[test]
    fn the_six_border_packets_become_one() {
        assert_eq!(
            merged_into(
                85,
                &clientbound::play::SET_BORDER_SIZE,
                &clientbound::play::WORLD_BORDER,
                &CSetBorderSize::new(16.0)
            ),
            [0, 0x40, 0x30, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            merged(
                86,
                &clientbound::play::SET_BORDER_LERP_SIZE,
                &CSetBorderLerpSize::new(0.0, 0.0, VarLong(5))
            ),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]
        );
        assert_eq!(
            merged(
                87,
                &clientbound::play::SET_BORDER_CENTER,
                &CSetBorderCenter::new(0.0, 0.0)
            )[0],
            2
        );
        let initialize = merged(
            88,
            &clientbound::play::INITIALIZE_BORDER,
            &CInitializeWorldBorder::new(
                0.0,
                0.0,
                1.0,
                2.0,
                VarLong(3),
                VarInt(4),
                VarInt(5),
                VarInt(6),
            ),
        );
        assert_eq!(initialize[0], 3);
        assert_eq!(&initialize[initialize.len() - 4..], &[3, 4, 5, 6]);
        assert_eq!(
            merged(
                89,
                &clientbound::play::SET_BORDER_WARNING_DELAY,
                &CSetBorderWarningDelay::new(VarInt(7))
            ),
            [4, 7]
        );
        assert_eq!(
            merged(
                90,
                &clientbound::play::SET_BORDER_WARNING_DISTANCE,
                &CSetBorderWarningDistance::new(VarInt(8))
            ),
            [5, 8]
        );
    }

    /// minecraft-data 1.16.2 `packet_combat_event`: an event varint, a
    /// duration or player varint, the opponent int and, for event 2, the
    /// message.
    #[test]
    fn the_three_combat_packets_become_one() {
        assert_eq!(
            merged_into(
                91,
                &clientbound::play::PLAYER_COMBAT_ENTER,
                &clientbound::play::COMBAT_EVENT,
                &CCombatEnter
            ),
            [0]
        );
        assert_eq!(
            merged(
                92,
                &clientbound::play::PLAYER_COMBAT_END,
                &CCombatEnd::new(VarInt(9))
            ),
            [1, 9, 0xff, 0xff, 0xff, 0xff]
        );

        let message = TextComponent::text("ouch");
        let kill = merged(
            93,
            &clientbound::play::PLAYER_COMBAT_KILL,
            &CCombatDeath::new(VarInt(2), &message),
        );
        assert_eq!(&kill[..6], &[2, 2, 0xff, 0xff, 0xff, 0xff]);
    }
}
