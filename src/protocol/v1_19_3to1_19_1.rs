use crate::api::rewriter::chat;
use crate::api::types::{
    ArrayT, BOOL, BYTE_ARRAY, I64T, OptionalT, STRING, TextComponentT, U8, UUID, VAR_INT,
};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;

const ADD_PLAYER: u8 = 0x01;
const INITIALIZE_CHAT: u8 = 0x02;
const UPDATE_GAME_MODE: u8 = 0x04;
const UPDATE_LISTED: u8 = 0x08;
const UPDATE_LATENCY: u8 = 0x10;
const UPDATE_DISPLAY_NAME: u8 = 0x20;

pub struct Protocol1_19_3To1_19_1;

impl Protocol for Protocol1_19_3To1_19_1 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_19_3,
            to: JavaMinecraftVersion::V_1_19_1,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // Chat preview disappeared in 1.19.3 and has no modern replacement.
        reg.cancel_serverbound(&serverbound::play::CHAT_PREVIEW);
        reg.serverbound_layout(&serverbound::play::CHAT, chat);
        reg.clientbound_layout(&clientbound::play::PLAYER_INFO_UPDATE, player_info_update);
        reg.clientbound_layout(&clientbound::play::PLAYER_INFO_REMOVE, player_info_remove);
        reg.clientbound_layout(&clientbound::play::PLAYER_CHAT, player_chat);
        reg.clientbound_layout(&clientbound::play::DISGUISED_CHAT, disguised_chat);
        reg.clientbound_layout(&clientbound::play::DELETE_CHAT, delete_chat);
    }
}

/// 1.19.3's signature is over a scheme too different to carry over, so it's left out;
/// core's pre-1.19.3 branch reads the signed preview byte in place of the counter.
fn chat(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&STRING)?;
    wrapper.passthrough(&I64T)?;
    wrapper.passthrough(&I64T)?;
    wrapper.read(&BYTE_ARRAY)?;
    wrapper.read(&BOOL)?;
    let seen = wrapper.read(&VAR_INT)?.0;
    if !(0..=5).contains(&seen) {
        return Err(TranslateError::Unsupported("last seen message count"));
    }
    for _ in 0..seen {
        wrapper.read(&UUID)?;
        wrapper.read(&BYTE_ARRAY)?;
    }
    if wrapper.read(&BOOL)? {
        wrapper.read(&UUID)?;
        wrapper.read(&BYTE_ARRAY)?;
    }
    wrapper.write(&BOOL, &false)?;
    wrapper.write(&U8, &0)?;
    Ok(())
}

/// One player's entry, gathered from whichever bits the mask set.
struct Profile {
    uuid: uuid::Uuid,
    name: Box<str>,
    properties: Vec<Property>,
    key: Option<ProfileKey>,
    game_mode: i32,
    latency: i32,
    display_name: Option<TextComponent>,
}

struct Property {
    name: Box<str>,
    value: Box<str>,
    signature: Option<Box<str>>,
}

struct ProfileKey {
    expires_at: i64,
    public_key: Vec<u8>,
    signature: Vec<u8>,
}

fn read_properties(wrapper: &mut PacketWrapper) -> Result<Vec<Property>, TranslateError> {
    let count = wrapper.read(&VAR_INT)?.0;
    if !(0..=16).contains(&count) {
        return Err(TranslateError::Unsupported("profile property count"));
    }
    let mut properties = Vec::with_capacity(count as usize);
    for _ in 0..count {
        properties.push(Property {
            name: wrapper.read(&STRING)?,
            value: wrapper.read(&STRING)?,
            signature: wrapper.read(&OptionalT(STRING))?,
        });
    }
    Ok(properties)
}

fn write_properties(
    wrapper: &mut PacketWrapper,
    properties: &[Property],
) -> Result<(), TranslateError> {
    wrapper.write(
        &VAR_INT,
        &VarInt(i32::try_from(properties.len()).unwrap_or(0)),
    )?;
    for property in properties {
        wrapper.write(&STRING, &property.name)?;
        wrapper.write(&STRING, &property.value)?;
        wrapper.write(&OptionalT(STRING), &property.signature)?;
    }
    Ok(())
}

fn read_profiles(
    wrapper: &mut PacketWrapper,
    actions: u8,
    text: TextComponentT,
) -> Result<Vec<Profile>, TranslateError> {
    let count = wrapper.read(&VAR_INT)?.0;
    if !(0..=1024).contains(&count) {
        return Err(TranslateError::Unsupported("player info entry count"));
    }
    let mut profiles = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut profile = Profile {
            uuid: wrapper.read(&UUID)?,
            name: "".into(),
            properties: Vec::new(),
            key: None,
            game_mode: 0,
            latency: 0,
            display_name: None,
        };
        if actions & ADD_PLAYER != 0 {
            profile.name = wrapper.read(&STRING)?;
            profile.properties = read_properties(wrapper)?;
        }
        if actions & INITIALIZE_CHAT != 0 && wrapper.read(&BOOL)? {
            wrapper.read(&UUID)?;
            profile.key = Some(ProfileKey {
                expires_at: wrapper.read(&I64T)?,
                public_key: wrapper.read(&BYTE_ARRAY)?,
                signature: wrapper.read(&BYTE_ARRAY)?,
            });
        }
        if actions & UPDATE_GAME_MODE != 0 {
            profile.game_mode = wrapper.read(&VAR_INT)?.0;
        }
        if actions & UPDATE_LISTED != 0 {
            wrapper.read(&BOOL)?;
        }
        if actions & UPDATE_LATENCY != 0 {
            profile.latency = wrapper.read(&VAR_INT)?.0;
        }
        if actions & UPDATE_DISPLAY_NAME != 0 {
            profile.display_name = wrapper.read(&OptionalT(text))?;
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

/// The bitmask packet collapses onto the single legacy action it stands for.
fn player_info_update(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let actions = wrapper.read(&U8)?;
    let known = ADD_PLAYER
        | INITIALIZE_CHAT
        | UPDATE_GAME_MODE
        | UPDATE_LISTED
        | UPDATE_LATENCY
        | UPDATE_DISPLAY_NAME;
    if actions & !known != 0 {
        return Err(TranslateError::Unsupported("player info action"));
    }
    let profiles = read_profiles(wrapper, actions, chat::text(connection))?;

    let action = if actions & ADD_PLAYER != 0 {
        0
    } else if actions & UPDATE_GAME_MODE != 0 {
        1
    } else if actions & UPDATE_LATENCY != 0 {
        2
    } else if actions & UPDATE_DISPLAY_NAME != 0 {
        3
    } else {
        wrapper.cancel();
        return Ok(());
    };

    let text = chat::text(connection);
    wrapper.write(&VAR_INT, &VarInt(action))?;
    wrapper.write(
        &VAR_INT,
        &VarInt(i32::try_from(profiles.len()).unwrap_or(0)),
    )?;
    for profile in &profiles {
        wrapper.write(&UUID, &profile.uuid)?;
        match action {
            0 => {
                wrapper.write(&STRING, &profile.name)?;
                write_properties(wrapper, &profile.properties)?;
                wrapper.write(&VAR_INT, &VarInt(profile.game_mode))?;
                wrapper.write(&VAR_INT, &VarInt(profile.latency))?;
                wrapper.write(&OptionalT(text), &profile.display_name)?;
                match &profile.key {
                    Some(key) => {
                        wrapper.write(&BOOL, &true)?;
                        wrapper.write(&I64T, &key.expires_at)?;
                        wrapper.write(&BYTE_ARRAY, &key.public_key)?;
                        wrapper.write(&BYTE_ARRAY, &key.signature)?;
                    }
                    None => wrapper.write(&BOOL, &false)?,
                }
            }
            1 => wrapper.write(&VAR_INT, &VarInt(profile.game_mode))?,
            2 => wrapper.write(&VAR_INT, &VarInt(profile.latency))?,
            _ => wrapper.write(&OptionalT(text), &profile.display_name)?,
        }
    }
    wrapper.set_packet(&clientbound::play::PLAYER_INFO);
    Ok(())
}

/// Removing a player is action 4 of the one packet 1.19.1 has.
fn player_info_remove(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let players = wrapper.read(&ArrayT(UUID))?;
    wrapper.write(&VAR_INT, &VarInt(4))?;
    wrapper.write(&ArrayT(UUID), &players)?;
    wrapper.set_packet(&clientbound::play::PLAYER_INFO);
    Ok(())
}

/// 1.19.1 signs chat by a scheme nothing here can fill in, so the message goes
/// out decorated as a system one, as ViaBackwards does.
fn player_chat(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    chat::player_chat_to_system(wrapper, chat::text(connection))?;
    wrapper.set_packet(&clientbound::play::SYSTEM_CHAT);
    Ok(())
}

/// 1.19.1 has no disguised chat at all.
fn disguised_chat(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    chat::disguised_chat_to_system(wrapper, chat::text(connection))?;
    wrapper.set_packet(&clientbound::play::SYSTEM_CHAT);
    Ok(())
}

fn delete_chat(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    chat::delete_chat_to_1_19_1(wrapper)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::WireType;
    use crate::pipeline::translate_serverbound;
    use pumpkin_protocol::ServerPacket;
    use pumpkin_protocol::java::server::play::SChatMessage;

    const PLAY: u8 = 5;

    /// `md('1.19').protocol.play.toServer.packet_chat_message`: message,
    /// timestamp, salt, a varint prefixed signature and the signed preview
    /// flag. 1.19.2 adds the last seen list and the last rejected message.
    fn sent_by_client(version: JavaMinecraftVersion) -> Vec<u8> {
        let mut bytes = Vec::new();
        STRING.write(&mut bytes, &"hello".into()).unwrap();
        I64T.write(&mut bytes, &7).unwrap();
        I64T.write(&mut bytes, &9).unwrap();
        BYTE_ARRAY.write(&mut bytes, &vec![0xab; 256]).unwrap();
        BOOL.write(&mut bytes, &false).unwrap();
        if version >= JavaMinecraftVersion::V_1_19_1 {
            VAR_INT
                .write(&mut bytes, &pumpkin_protocol::codec::var_int::VarInt(1))
                .unwrap();
            UUID.write(&mut bytes, &uuid::Uuid::from_u128(3)).unwrap();
            BYTE_ARRAY.write(&mut bytes, &vec![0xcd; 256]).unwrap();
            BOOL.write(&mut bytes, &true).unwrap();
            UUID.write(&mut bytes, &uuid::Uuid::from_u128(4)).unwrap();
            BYTE_ARRAY.write(&mut bytes, &vec![0xef; 256]).unwrap();
        }
        bytes
    }

    fn read_by_core(version: JavaMinecraftVersion) {
        let out = translate_serverbound(
            0,
            version,
            PLAY,
            serverbound::play::CHAT.to_id(version),
            &sent_by_client(version),
        )
        .unwrap();
        let mut read: &[u8] = &out.payload;
        let packet = SChatMessage::read(&mut read, &version).unwrap();
        assert!(read.is_empty(), "{version}");
        assert_eq!(packet.message, "hello", "{version}");
        assert_eq!(packet.timestamp, 7, "{version}");
        assert_eq!(packet.salt, 9, "{version}");
        assert!(packet.signature.is_none(), "{version}");
    }

    #[test]
    fn the_signed_chat_forms_below_1_19_3_reach_core() {
        read_by_core(JavaMinecraftVersion::V_1_19);
        read_by_core(JavaMinecraftVersion::V_1_19_1);
    }

    /// Below 1.19 the packet is the message alone and core reads it as sent.
    #[test]
    fn a_plain_message_is_left_alone() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let mut payload = Vec::new();
        STRING.write(&mut payload, &"hello".into()).unwrap();
        let out = translate_serverbound(
            0,
            version,
            PLAY,
            serverbound::play::CHAT.to_id(version),
            &payload,
        )
        .unwrap();
        assert_eq!(out.payload, payload);
    }
}

#[cfg(test)]
mod player_tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::pipeline::translate_clientbound;
    use pumpkin_protocol::Property;
    use pumpkin_protocol::java::client::play::{
        CDeleteChat, CPlayerChatMessage, CPlayerInfoUpdate, CRemovePlayerInfo, FilterType,
        Player as InfoPlayer, PlayerAction,
    };
    use pumpkin_protocol::ser::NetworkReadExt;
    use pumpkin_protocol::{ClientPacket, VarInt as ProtocolVarInt};
    use pumpkin_util::text::TextComponent;
    const PLAY: u8 = 5;
    const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_1_19_1;
    const UUID_BYTES: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7];

    fn update(actions: u8, entry: &[PlayerAction]) -> Vec<u8> {
        let players = [InfoPlayer {
            uuid: uuid::Uuid::from_bytes(UUID_BYTES),
            actions: entry,
        }];
        let mut payload = Vec::new();
        CPlayerInfoUpdate::new(actions, &players)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();
        payload
    }

    /// minecraft-data 1.19.2 `packet_player_info`: an action varint, an entry
    /// array, and for `add_player` a name, a property array, gamemode, ping,
    /// an optional display name and an optional chat key.
    #[test]
    fn an_added_player_keeps_everything_the_other_bits_carried() {
        let properties = [Property {
            name: "textures".into(),
            value: "v".into(),
            signature: None,
        }];
        let payload = update(
            ADD_PLAYER | UPDATE_GAME_MODE | UPDATE_LISTED | UPDATE_LATENCY,
            &[
                PlayerAction::AddPlayer {
                    name: "bob",
                    properties: &properties,
                },
                PlayerAction::UpdateGameMode(ProtocolVarInt(1)),
                PlayerAction::UpdateListed(true),
                PlayerAction::UpdateLatency(ProtocolVarInt(42)),
            ],
        );

        let out = translate_clientbound(
            40,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_INFO_UPDATE.v26_3,
            &payload,
        )
        .unwrap();
        let mut expected = vec![0x00, 0x01];
        expected.extend_from_slice(&UUID_BYTES);
        expected.extend_from_slice(b"\x03bob\x01\x08textures\x01v\x00");
        expected.extend_from_slice(&[0x01, 0x2a, 0x00, 0x00]);
        assert_eq!(out.payload, expected);
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::PLAYER_INFO.to_id(VERSION)
        );
        remove_connection(40);
    }

    /// Only the latency bit is set, so the legacy packet is action 2.
    #[test]
    fn a_latency_only_update_becomes_action_two() {
        let payload = update(
            UPDATE_LATENCY,
            &[PlayerAction::UpdateLatency(ProtocolVarInt(9))],
        );
        let out = translate_clientbound(
            41,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_INFO_UPDATE.v26_3,
            &payload,
        )
        .unwrap();
        let mut expected = vec![0x02, 0x01];
        expected.extend_from_slice(&UUID_BYTES);
        expected.push(0x09);
        assert_eq!(out.payload, expected);
        remove_connection(41);
    }

    /// Nothing the legacy packet can express, so it is not sent at all.
    #[test]
    fn a_listed_only_update_is_dropped() {
        let payload = update(UPDATE_LISTED, &[PlayerAction::UpdateListed(false)]);
        assert!(
            translate_clientbound(
                42,
                VERSION,
                PLAY,
                clientbound::play::PLAYER_INFO_UPDATE.v26_3,
                &payload
            )
            .is_none()
        );
        remove_connection(42);
    }

    #[test]
    fn removing_a_player_is_action_four() {
        let players = [uuid::Uuid::from_bytes(UUID_BYTES)];
        let mut payload = Vec::new();
        CRemovePlayerInfo::new(&players)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            43,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_INFO_REMOVE.v26_3,
            &payload,
        )
        .unwrap();
        let mut expected = vec![0x04, 0x01];
        expected.extend_from_slice(&UUID_BYTES);
        assert_eq!(out.payload, expected);
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::PLAYER_INFO.to_id(VERSION)
        );
        remove_connection(43);
    }

    /// minecraft-data 1.19.2 `packet_system_chat`: the message and the
    /// actionbar flag.
    #[test]
    fn a_signed_message_arrives_decorated_as_a_system_one() {
        let sender = TextComponent::text("bob");
        let chat = CPlayerChatMessage::new(
            ProtocolVarInt(0),
            uuid::Uuid::from_bytes(UUID_BYTES),
            ProtocolVarInt(0),
            None,
            "hello".into(),
            0,
            0,
            Box::new([]),
            None,
            FilterType::PassThrough,
            // The raw chat type, which the server sends as the holder 8.
            ProtocolVarInt(8),
            sender,
            None,
        );
        let mut payload = Vec::new();
        chat.write_packet_data(&mut payload, &VERSION).unwrap();

        let out = translate_clientbound(
            44,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_CHAT.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::SYSTEM_CHAT.to_id(VERSION)
        );
        let mut cursor: &[u8] = &out.payload;
        let json = cursor.get_str().unwrap();
        assert!(json.contains("hello"), "{json}");
        assert!(!cursor.get_bool().unwrap());
        assert!(cursor.is_empty());
        remove_connection(44);
    }

    /// minecraft-data 1.19.2 `packet_hide_message`: the whole signature as one
    /// length prefixed array.
    #[test]
    fn a_deleted_message_carries_its_whole_signature() {
        let signature = [7u8; 256];
        let mut payload = Vec::new();
        CDeleteChat::from_signature(&signature)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            45,
            VERSION,
            PLAY,
            clientbound::play::DELETE_CHAT.v26_3,
            &payload,
        )
        .unwrap();
        let mut expected = vec![0x80, 0x02];
        expected.extend_from_slice(&signature);
        assert_eq!(out.payload, expected);
        remove_connection(45);
    }

    /// A cached signature has no 1.19.2 form, so the packet is dropped.
    #[test]
    fn a_cached_signature_id_is_dropped() {
        let mut payload = Vec::new();
        CDeleteChat::from_cache_id(3)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();
        assert!(
            translate_clientbound(
                46,
                VERSION,
                PLAY,
                clientbound::play::DELETE_CHAT.v26_3,
                &payload
            )
            .is_none()
        );
        remove_connection(46);
    }

    #[test]
    fn removed_chat_preview_is_explicitly_cancelled() {
        let protocol = Protocol1_19_3To1_19_1;
        let packet = &serverbound::play::CHAT_PREVIEW;
        assert_eq!(packet.v26_3, -1);

        let mut registry = Registry::default();
        protocol.register(&mut registry);
        let handler = registry
            .serverbound_handler(packet)
            .expect("removed packet must have a cancellation handler")
            .handler;
        for (key, version) in [
            (47, JavaMinecraftVersion::V_1_19_1),
            (48, JavaMinecraftVersion::V_1_19),
        ] {
            assert_ne!(packet.to_id(version), -1, "{version}");
            let mut wrapper = PacketWrapper::new(packet, &[]);
            let mut connection = UserConnection::new(key, version);
            let context = Ctx {
                step: protocol.step(),
                mappings: MappingData::get().step(JavaMinecraftVersion::V_1_19_3),
                layout: version,
            };
            handler(&mut wrapper, &mut connection, &context).unwrap();
            assert!(wrapper.finish().unwrap().is_none(), "{version}");
        }
    }
}
