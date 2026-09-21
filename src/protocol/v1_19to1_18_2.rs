use crate::api::rewriter::chat;
use crate::api::rewriter::entity::{read_spawn, replace, spawned_type};
use crate::api::types::{
    BOOL, BYTE_ARRAY, F32T, I8, I32T, I64T, ItemT, OptionalT, STRING, U8, UUID, VAR_INT,
};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::legacy::{
    CSpawnLivingEntity, CSpawnPainting, DEFAULT_VARIANT, direction_2d_from_3d_index,
};
use crate::packet::mappings::clientbound;
use pumpkin_data::entity::EntityType;
use pumpkin_protocol::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::version::JavaMinecraftVersion;

pub struct Protocol1_19To1_18_2;

impl Protocol for Protocol1_19To1_18_2 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_19,
            to: JavaMinecraftVersion::V_1_18_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::ADD_ENTITY, add_entity);
        reg.clientbound(&clientbound::play::MERCHANT_OFFERS, merchant_offers);
        reg.clientbound(&clientbound::play::SYSTEM_CHAT, system_chat);
        reg.clientbound_layout(&clientbound::play::PLAYER_INFO, player_info);
    }
}

/// A flag says whether a second cost follows below 1.19; core writes the stack
/// alone, which the client reads as the flag and then as a slot of its own.
fn merchant_offers(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let item = ItemT::for_version(ctx.layout);
    wrapper.passthrough(&VAR_INT)?;
    let offers = wrapper.passthrough(&U8)?;
    for _ in 0..offers {
        wrapper.passthrough(&item)?;
        wrapper.passthrough(&item)?;
        let cost_b = wrapper.read(&item)?;
        wrapper.write(&BOOL, &!cost_b.is_empty())?;
        if !cost_b.is_empty() {
            wrapper.write(&item, &cost_b)?;
        }
        wrapper.passthrough(&BOOL)?;
        for _ in 0..4 {
            wrapper.passthrough(&I32T)?;
        }
        wrapper.passthrough(&F32T)?;
        wrapper.passthrough(&I32T)?;
    }
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&BOOL)?;
    wrapper.passthrough(&BOOL)?;
    Ok(())
}

/// Vanilla sends every `LivingEntity` but the player through
/// `ClientboundAddMobPacket` here, and `EntityType::mob` leaves out the armor
/// stand.
fn spawns_as_living(entity_type: u16) -> bool {
    entity_type == EntityType::ARMOR_STAND.id
        || EntityType::from_raw(entity_type).is_some_and(|kind| kind.mob)
}

fn add_entity(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let Some(entity_type) = spawned_type(wrapper, connection) else {
        wrapper.passthrough_all();
        return Ok(());
    };

    if entity_type == EntityType::PAINTING.id {
        let spawn = read_spawn(wrapper, ctx.layout)?;
        // 26.3 puts the 3D facing in the data field and the variant in metadata.
        let direction = direction_2d_from_3d_index(spawn.data.0)
            .ok_or(TranslateError::Unsupported("painting facing"))?;
        let painting = CSpawnPainting::new(
            spawn.entity_id,
            spawn.entity_uuid,
            String::new(),
            DEFAULT_VARIANT,
            BlockPos::new(
                spawn.position.x.floor() as i32,
                spawn.position.y.floor() as i32,
                spawn.position.z.floor() as i32,
            ),
            direction,
        );
        return replace(
            wrapper,
            &clientbound::play::SPAWN_PAINTING,
            &painting,
            ctx.layout,
        );
    }

    if spawns_as_living(entity_type) {
        let spawn = read_spawn(wrapper, ctx.layout)?;
        let living = CSpawnLivingEntity::new(
            spawn.entity_id,
            spawn.entity_uuid,
            VarInt(i32::from(entity_type)),
            spawn.position,
            spawn.pitch_degrees(),
            spawn.yaw_degrees(),
            spawn.head_yaw_degrees(),
            spawn.velocity.0,
            None,
        );
        return replace(
            wrapper,
            &clientbound::play::SPAWN_LIVING_ENTITY,
            &living,
            ctx.layout,
        );
    }

    wrapper.passthrough_all();
    Ok(())
}

/// 1.18.2 has one chat packet. Core writes its body itself for a client it
/// knows, so only a payload that came down the chain still needs converting.
fn system_chat(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout >= JavaMinecraftVersion::V_1_19 {
        wrapper.passthrough(&chat::text(connection))?;
        let destination = wrapper.read(&VAR_INT)?.0;
        wrapper.write(&I8, &if destination == 2 { 2 } else { 1 })?;
        wrapper.write(&UUID, &uuid::Uuid::nil())?;
    } else {
        wrapper.passthrough_all();
    }
    wrapper.set_packet(&clientbound::play::CHAT);
    Ok(())
}

/// 1.19 added the public key an added player signs chat with.
fn player_info(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let action = wrapper.passthrough(&VAR_INT)?.0;
    let count = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=1024).contains(&count) {
        return Err(TranslateError::Unsupported("player info entry count"));
    }
    let text = OptionalT(chat::text(connection));
    for _ in 0..count {
        wrapper.passthrough(&UUID)?;
        match action {
            0 => {
                wrapper.passthrough(&STRING)?;
                let properties = wrapper.passthrough(&VAR_INT)?.0;
                if !(0..=16).contains(&properties) {
                    return Err(TranslateError::Unsupported("profile property count"));
                }
                for _ in 0..properties {
                    wrapper.passthrough(&STRING)?;
                    wrapper.passthrough(&STRING)?;
                    wrapper.passthrough(&OptionalT(STRING))?;
                }
                wrapper.passthrough(&VAR_INT)?;
                wrapper.passthrough(&VAR_INT)?;
                wrapper.passthrough(&text)?;
                if wrapper.read(&BOOL)? {
                    wrapper.read(&I64T)?;
                    wrapper.read(&BYTE_ARRAY)?;
                    wrapper.read(&BYTE_ARRAY)?;
                }
            }
            1 | 2 => {
                wrapper.passthrough(&VAR_INT)?;
            }
            3 => {
                wrapper.passthrough(&text)?;
            }
            4 => {}
            _ => return Err(TranslateError::Unsupported("player info action")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::spawns_as_living;
    use crate::api::types::{BOOL, F32T, I32T, ItemT, U8, VAR_INT, WireType};
    use crate::packet::mappings::clientbound;
    use crate::pipeline::translate_clientbound;
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::java::client::play::{CMerchantOffers, MerchantOffer};
    use pumpkin_util::version::JavaMinecraftVersion;

    const PLAY: u8 = 5;

    fn offer(cost_b: Option<&'static Item>) -> MerchantOffer {
        MerchantOffer {
            base_cost_a: ItemStackSerializer::from(ItemStack::new(12, &Item::EMERALD)),
            output: ItemStackSerializer::from(ItemStack::new(1, &Item::BOOK)),
            cost_b: cost_b.map(|item| ItemStackSerializer::from(ItemStack::new(3, item))),
            reward_exp: true,
            uses: 0,
            max_uses: 12,
            xp: 1,
            special_price: 0,
            price_multiplier: 0.05,
            demand: 0,
        }
    }

    fn written_by_core(version: JavaMinecraftVersion) -> Vec<u8> {
        let packet = CMerchantOffers::new(
            VarInt(1),
            vec![offer(None), offer(Some(&Item::EMERALD))],
            VarInt(2),
            VarInt(7),
            true,
            false,
        );
        let mut bytes = Vec::new();
        packet.write_packet_data(&mut bytes, &version).unwrap();
        bytes
    }

    /// `md('1.18.2').protocol.play.toClient.packet_trade_list` types
    /// `inputItem2` as `["option","slot"]`; on 1.19 it is a bare slot.
    #[test]
    fn a_second_cost_gets_its_flag_back_below_1_19() {
        let version = JavaMinecraftVersion::V_1_18_2;
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::MERCHANT_OFFERS.v26_3,
            &written_by_core(version),
        )
        .unwrap();

        let mut read: &[u8] = &out.payload;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(U8.read(&mut read).unwrap(), 2, "a byte of trade count");
        for expected in [false, true] {
            read_offer(&mut read, version, expected);
        }
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 7);
        assert!(BOOL.read(&mut read).unwrap());
        assert!(!BOOL.read(&mut read).unwrap());
        assert!(read.is_empty());
    }

    fn read_offer(read: &mut &[u8], version: JavaMinecraftVersion, has_cost_b: bool) {
        let item = ItemT::for_version(version);
        item.read(read).unwrap();
        item.read(read).unwrap();
        assert_eq!(BOOL.read(read).unwrap(), has_cost_b, "the flag");
        if has_cost_b {
            assert!(item.read(read).unwrap().item_id().is_some());
        }
        BOOL.read(read).unwrap();
        for _ in 0..4 {
            I32T.read(read).unwrap();
        }
        F32T.read(read).unwrap();
        I32T.read(read).unwrap();
    }

    /// From 1.19 the second cost is a bare slot again, so the frame core wrote
    /// reaches the client untouched.
    #[test]
    fn a_second_cost_keeps_the_bare_slot_from_1_19() {
        let version = JavaMinecraftVersion::V_1_19;
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::MERCHANT_OFFERS.v26_3,
            &written_by_core(version),
        )
        .unwrap();

        let item = ItemT::for_version(version);
        let mut read: &[u8] = &out.payload;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2, "a varint count");
        for _ in 0..2 {
            for _ in 0..3 {
                item.read(&mut read).unwrap();
            }
            BOOL.read(&mut read).unwrap();
            for _ in 0..4 {
                I32T.read(&mut read).unwrap();
            }
            F32T.read(&mut read).unwrap();
            I32T.read(&mut read).unwrap();
        }
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2);
    }

    #[test]
    fn living_types_include_armor_stands_but_not_objects() {
        assert!(spawns_as_living(EntityType::ARMOR_STAND.id));
        assert!(spawns_as_living(EntityType::PIG.id));
        assert!(spawns_as_living(EntityType::VILLAGER.id));
        assert!(!spawns_as_living(EntityType::ARROW.id));
        assert!(!spawns_as_living(EntityType::ITEM.id));
        assert!(!spawns_as_living(EntityType::OAK_BOAT.id));
        assert!(!spawns_as_living(EntityType::PAINTING.id));
        assert!(!spawns_as_living(EntityType::PLAYER.id));
    }
}

#[cfg(test)]
mod player_tests {
    use super::spawns_as_living;
    use crate::api::remove_connection;
    use crate::packet::mappings::clientbound;
    use crate::pipeline::translate_clientbound;
    use pumpkin_data::entity::EntityType;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::java::client::play::{
        CDisguisedChatMessage, CPlayerChatMessage, CPlayerInfoUpdate, CSystemChatMessage,
        FilterType, InitChat, Player as InfoPlayer, PlayerAction,
    };
    use pumpkin_protocol::ser::NetworkReadExt;
    use pumpkin_util::text::TextComponent;
    use pumpkin_util::version::JavaMinecraftVersion;
    const PLAY: u8 = 5;
    const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_1_16_2;

    /// minecraft-data 1.16.2 `packet_chat`: a json message, a position byte
    /// and the sender uuid.
    fn read_legacy_chat(payload: &[u8]) -> (String, i8, uuid::Uuid) {
        let mut cursor = payload;
        let message = cursor.get_str().unwrap().to_string();
        let position = cursor.get_i8().unwrap();
        let sender = cursor.get_uuid().unwrap();
        assert!(cursor.is_empty());
        (message, position, sender)
    }

    /// Core writes the legacy body itself, so the packet only changes id.
    #[test]
    fn a_system_message_arrives_as_the_one_chat_packet() {
        let content = TextComponent::text("hello");
        let mut payload = Vec::new();
        CSystemChatMessage::new(&content, false)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            50,
            VERSION,
            PLAY,
            clientbound::play::SYSTEM_CHAT.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(out.payload, payload);
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::CHAT.to_id(VERSION)
        );
        remove_connection(50);
    }

    /// A disguised chat has to cross every boundary down to the one packet:
    /// the holder at 1.21, the decoration at 1.19.3, the destination at 1.19.
    #[test]
    fn a_disguised_message_is_decorated_and_folded_into_chat() {
        let message = TextComponent::text("hello");
        let sender = TextComponent::text("bob");
        let mut payload = Vec::new();
        // The raw chat type, which the server sends as the holder 8.
        CDisguisedChatMessage::new(&message, VarInt(8), &sender, None)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            51,
            VERSION,
            PLAY,
            clientbound::play::DISGUISED_CHAT.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::CHAT.to_id(VERSION)
        );
        let (json, position, sender_uuid) = read_legacy_chat(&out.payload);
        assert!(json.contains("hello"), "{json}");
        assert_eq!(position, 1);
        assert!(sender_uuid.is_nil());
        remove_connection(51);
    }

    /// 1.18.2 knows nothing of the chat key an added player carries from 1.19,
    /// so the whole way down is 1.19.3 to the legacy action and then the strip.
    #[test]
    fn an_added_player_loses_its_chat_key() {
        let actions = [
            PlayerAction::AddPlayer {
                name: "bob",
                properties: &[],
            },
            PlayerAction::InitializeChat(Some(InitChat {
                session_id: uuid::Uuid::nil(),
                expires_at: 5,
                public_key: Box::new([0xaa]),
                signature: Box::new([0xbb, 0xcc]),
            })),
        ];
        let players = [InfoPlayer {
            uuid: uuid::Uuid::nil(),
            actions: &actions,
        }];
        let mut payload = Vec::new();
        CPlayerInfoUpdate::new(0x01 | 0x02, &players)
            .write_packet_data(&mut payload, &VERSION)
            .unwrap();

        let out = translate_clientbound(
            52,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_INFO_UPDATE.v26_3,
            &payload,
        )
        .unwrap();
        let mut expected = vec![0x00, 0x01];
        expected.extend_from_slice(&[0u8; 16]);
        expected.extend_from_slice(b"bob    ");
        assert_eq!(out.payload, expected);
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::PLAYER_INFO.to_id(VERSION)
        );
        remove_connection(52);
    }

    /// The whole chain: the global index goes at 1.21.5, the holder at 1.21,
    /// the signed form at 1.19.3 and the destination at 1.19.
    #[test]
    fn a_signed_message_reaches_1_16_2_as_chat() {
        let mut payload = Vec::new();
        CPlayerChatMessage::new(
            VarInt(0),
            uuid::Uuid::nil(),
            VarInt(0),
            None,
            "hello".into(),
            0,
            0,
            Box::new([]),
            None,
            FilterType::PassThrough,
            VarInt(8),
            TextComponent::text("bob"),
            None,
        )
        .write_packet_data(&mut payload, &VERSION)
        .unwrap();

        let out = translate_clientbound(
            53,
            VERSION,
            PLAY,
            clientbound::play::PLAYER_CHAT.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(
            out.packet.to_id(VERSION),
            clientbound::play::CHAT.to_id(VERSION)
        );
        let (json, position, sender) = read_legacy_chat(&out.payload);
        assert!(json.contains("hello"), "{json}");
        assert_eq!(position, 1);
        assert!(sender.is_nil());
        remove_connection(53);
    }
}
