//! ViaBackwards' 1.16.2 -> 1.16.1 protocol step.
//!
//! This step adapts 1.16.2 payload changes for a 1.16.1 client. Pumpkin's
//! negotiated-version login and respawn writers already emit the native
//! 1.16.1 dimension and game-mode fields; their conditional handlers below
//! only apply when an earlier stage supplies a 1.16.2 payload.

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::codec::var_long::VarLong;
use pumpkin_protocol::ser::NetworkReadExt;
use pumpkin_protocol::{ClientPacket, java::client::play::CSpawnEntity};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::chat;
use crate::api::types::{
    BOOL, F32T, F64T, I16T, I32T, I64T, NbtT, STRING, TextComponentT, U8, U8T, VAR_INT, WireType,
};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};

const MAX_COMMAND_NODES: i32 = 65_536;
const MAX_BLOCK_CHANGES: i32 = 4_096;

pub struct Protocol1_16_2To1_16_1;

impl Protocol for Protocol1_16_2To1_16_1 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_16_2,
            to: V::V_1_16_1,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // Pumpkin's CLogin/CRespawn writers branch on the negotiated client
        // version and already serialize these two packets in 1.16.1 form.
        // Keep these as layout handlers: Registered::runs skips them when the
        // core layout is native 1.16.1, so they cannot reinterpret those bytes
        // as the 1.16.2 dimension-codec form.
        reg.clientbound_layout(&clientbound::play::LOGIN, login);
        reg.clientbound_layout(&clientbound::play::RESPAWN, respawn);
        reg.clientbound_layout(&clientbound::play::CHAT, chat_message);
        // These handlers consume the client's native 1.16.x payload shape
        // (or the old packet alias, which Pumpkin currently does not emit).
        // Register semantic rewrites so they still run when layout is 1.16.1.
        reg.clientbound(&clientbound::play::COMMANDS, commands);
        reg.clientbound(&clientbound::play::UNLOCK_RECIPES, recipe_packet);
        reg.clientbound(
            &clientbound::play::SECTION_BLOCKS_UPDATE,
            section_blocks_update,
        );
        reg.clientbound(&clientbound::play::BLOCK_ENTITY_DATA, block_entity_data);
        reg.clientbound(&clientbound::play::ADD_ENTITY, track_piglin_stand_in);
        reg.clientbound(&clientbound::play::SET_ENTITY_DATA, piglin_metadata);
        reg.serverbound(&serverbound::play::RECIPE_BOOK_DATA, recipe_book_update);
    }
}

/// 1.16.2 added the hardcore bit ahead of game mode, changed max players to a
/// VarInt and inlined the dimension type. 1.16.1 expects the older game-mode
/// byte, an unsigned max-player byte and a dimension name. Via also replaces
/// the dimension codec with the 1.16 defaults. Custom dimension effects fall
/// back to the overworld, as in ViaBackwards.
fn login(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I32T)?;
    let hardcore = wrapper.read(&BOOL)?;
    let mut game_mode = wrapper.read(&U8T)?;
    if hardcore {
        game_mode |= 0x08;
    }
    wrapper.write(&U8T, &game_mode)?;
    wrapper.passthrough(&U8T)?; // Previous game mode.

    let worlds = wrapper.read(&VAR_INT)?.0;
    if !(0..=1024).contains(&worlds) {
        return Err(TranslateError::Unsupported("login world list"));
    }
    wrapper.write(&VAR_INT, &VarInt(worlds))?;
    for _ in 0..worlds {
        wrapper.passthrough(&STRING)?;
    }

    // 1.16.1's `dimension` list codec is not the 1.16.2 registry codec.
    wrapper.read(&NbtT::for_version(ctx.layout))?;
    wrapper.write(
        &NbtT::for_version(ctx.layout),
        &Some(legacy_dimension_registry()),
    )?;
    let dimension_data = wrapper.read(&NbtT::for_version(ctx.layout))?;
    wrapper.write(&STRING, &legacy_dimension(&dimension_data))?;
    wrapper.passthrough(&STRING)?; // World name.
    wrapper.passthrough(&I64T)?; // Hashed seed.

    let max_players = wrapper.read(&VAR_INT)?.0;
    let max_players = u8::try_from(max_players.clamp(0, i32::from(u8::MAX))).unwrap_or(u8::MAX);
    wrapper.write(&U8T, &max_players)?;
    wrapper.passthrough_all();
    Ok(())
}

fn respawn(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let dimension_data = wrapper.read(&NbtT::for_version(ctx.layout))?;
    wrapper.write(&STRING, &legacy_dimension(&dimension_data))?;
    wrapper.passthrough_all();
    Ok(())
}

fn legacy_dimension(dimension_data: &Option<NbtTag>) -> Box<str> {
    let effects = match dimension_data {
        Some(NbtTag::Compound(compound)) => compound.get_string("effects"),
        _ => None,
    };
    match effects {
        Some("minecraft:overworld" | "overworld") => "minecraft:overworld",
        Some("minecraft:the_nether" | "the_nether") => "minecraft:the_nether",
        Some("minecraft:the_end" | "the_end") => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
    .into()
}

fn legacy_dimension_registry() -> NbtTag {
    let overworld = shared_overworld("minecraft:overworld", false);
    let caves = shared_overworld("minecraft:overworld_caves", true);

    let mut nether = NbtCompound::new();
    nether.put_string("name", "minecraft:the_nether".to_owned());
    nether.put_byte("has_ceiling", 1);
    nether.put_byte("piglin_safe", 1);
    nether.put_byte("natural", 0);
    nether.put_float("ambient_light", 0.1);
    nether.put_string("infiniburn", "minecraft:infiniburn_nether".to_owned());
    nether.put_byte("respawn_anchor_works", 1);
    nether.put_byte("has_skylight", 0);
    nether.put_byte("bed_works", 0);
    nether.put_long("fixed_time", 18_000);
    nether.put_byte("has_raids", 0);
    nether.put_int("logical_height", 128);
    nether.put_byte("shrunk", 1);
    nether.put_byte("ultrawarm", 1);

    let mut end = NbtCompound::new();
    end.put_string("name", "minecraft:the_end".to_owned());
    end.put_byte("has_ceiling", 0);
    end.put_byte("piglin_safe", 0);
    end.put_byte("natural", 0);
    end.put_float("ambient_light", 0.0);
    end.put_string("infiniburn", "minecraft:infiniburn_end".to_owned());
    end.put_byte("respawn_anchor_works", 0);
    end.put_byte("has_skylight", 0);
    end.put_byte("bed_works", 0);
    end.put_long("fixed_time", 6_000);
    end.put_byte("has_raids", 1);
    end.put_int("logical_height", 256);
    end.put_byte("shrunk", 0);
    end.put_byte("ultrawarm", 0);

    let mut registry = NbtCompound::new();
    registry.put_list(
        "dimension",
        vec![
            NbtTag::Compound(overworld),
            NbtTag::Compound(caves),
            NbtTag::Compound(nether),
            NbtTag::Compound(end),
        ],
    );
    NbtTag::Compound(registry)
}

fn shared_overworld(name: &str, has_ceiling: bool) -> NbtCompound {
    let mut overworld = NbtCompound::new();
    overworld.put_string("name", name.to_owned());
    overworld.put_byte("has_ceiling", i8::from(has_ceiling));
    overworld.put_byte("piglin_safe", 0);
    overworld.put_byte("natural", 1);
    overworld.put_float("ambient_light", 0.0);
    overworld.put_string("infiniburn", "minecraft:infiniburn_overworld".to_owned());
    overworld.put_byte("respawn_anchor_works", 0);
    overworld.put_byte("has_skylight", 1);
    overworld.put_byte("bed_works", 1);
    overworld.put_byte("has_raids", 1);
    overworld.put_int("logical_height", 256);
    overworld.put_byte("shrunk", 0);
    overworld.put_byte("ultrawarm", 0);
    overworld
}

/// 1.16.2's chat packet adds no behavior the old client can use for an action
/// bar. ViaBackwards routes position 2 through the legacy title packet.
/// PJM's text API preserves translatable components, but does not carry Via's
/// locale translation table, so translation-key remapping is not attempted.
fn chat_message(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let text = chat::text(connection);
    let message = wrapper.read(&text)?;
    let position = wrapper.read(&U8T)?;
    if position == 2 {
        wrapper.set_packet(&clientbound::play::TITLE);
        wrapper.write(&VAR_INT, &VarInt(2))?;
        wrapper.write(&text, &message)?;
        wrapper.consume_remaining();
    } else {
        wrapper.write(&text, &message)?;
        wrapper.write(&U8T, &position)?;
        wrapper.passthrough_all();
    }
    Ok(())
}

/// Mirrors ViaVersion's command parser property readers and the ViaBackwards
/// angle -> single-word string substitution. Unknown parsers have no extra
/// fields in Via's generic rewriter and are copied as identifiers.
fn commands(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let nodes = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=MAX_COMMAND_NODES).contains(&nodes) {
        return Err(TranslateError::Unsupported("command node count"));
    }
    for _ in 0..nodes {
        let flags = wrapper.passthrough(&U8)?;
        let children = wrapper.passthrough(&VAR_INT)?.0;
        if !(0..=MAX_COMMAND_NODES).contains(&children) {
            return Err(TranslateError::Unsupported("command child count"));
        }
        for _ in 0..children {
            wrapper.passthrough(&VAR_INT)?;
        }
        if flags & 0x08 != 0 {
            wrapper.passthrough(&VAR_INT)?;
        }
        let node_type = flags & 0x03;
        if node_type == 1 || node_type == 2 {
            wrapper.passthrough(&STRING)?;
        }
        if node_type == 2 {
            let parser = wrapper.read(&STRING)?;
            if parser.as_ref() == "minecraft:angle" {
                wrapper.write(&STRING, &"brigadier:string".into())?;
                wrapper.write(&VAR_INT, &VarInt(0))?;
            } else {
                wrapper.write(&STRING, &parser)?;
                parser_properties(wrapper, &parser)?;
            }
        }
        if flags & 0x10 != 0 {
            wrapper.passthrough(&STRING)?;
        }
    }
    wrapper.passthrough(&VAR_INT)?; // Root node.
    Ok(())
}

fn parser_properties(wrapper: &mut PacketWrapper, parser: &str) -> Result<(), TranslateError> {
    match parser {
        "brigadier:double" => number_properties(wrapper, &F64T),
        "brigadier:float" => number_properties(wrapper, &F32T),
        "brigadier:integer" => number_properties(wrapper, &I32T),
        "brigadier:long" => number_properties(wrapper, &I64T),
        "brigadier:string" => {
            wrapper.passthrough(&VAR_INT)?;
            Ok(())
        }
        "minecraft:entity" | "minecraft:score_holder" => {
            wrapper.passthrough(&U8)?;
            Ok(())
        }
        "minecraft:resource"
        | "minecraft:resource_or_tag"
        | "minecraft:resource_or_tag_key"
        | "minecraft:resource_key"
        | "minecraft:resource_selector" => {
            wrapper.passthrough(&STRING)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn number_properties<T>(wrapper: &mut PacketWrapper, number: &T) -> Result<(), TranslateError>
where
    T: WireType,
    T::Value: Copy,
{
    let flags = wrapper.passthrough(&U8)?;
    if flags & 1 != 0 {
        wrapper.passthrough(number)?;
    }
    if flags & 2 != 0 {
        wrapper.passthrough(number)?;
    }
    Ok(())
}

/// The old recipe-book packet carries one "shown" recipe or the settings for
/// all three categories. The newer server expects one packet per category.
fn recipe_book_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let kind = wrapper.read(&VAR_INT)?.0;
    if kind == 0 {
        let recipe = wrapper.read(&STRING)?;
        wrapper.set_packet(&serverbound::play::RECIPE_BOOK_SEEN_RECIPE);
        wrapper.write(&STRING, &recipe)?;
        return Ok(());
    }

    let mut settings = Vec::with_capacity(3);
    for recipe_type in 0..3 {
        let open = wrapper.read(&BOOL)?;
        let filter = wrapper.read(&BOOL)?;
        settings.push(recipe_setting_payload(recipe_type, open, filter)?);
    }
    wrapper.consume_remaining();
    wrapper.cancel();
    for payload in settings {
        wrapper.send_serverbound(&serverbound::play::RECIPE_BOOK_CHANGE_SETTINGS, payload);
    }
    Ok(())
}

fn recipe_setting_payload(
    recipe_type: i32,
    open: bool,
    filter: bool,
) -> Result<Vec<u8>, TranslateError> {
    let mut payload = Vec::with_capacity(3);
    VAR_INT.write(&mut payload, &VarInt(recipe_type))?;
    BOOL.write(&mut payload, &open)?;
    BOOL.write(&mut payload, &filter)?;
    Ok(payload)
}

/// 1.16.2's recipe-book settings packet has four new Blast Furnace and Smoker
/// booleans; the 1.16.1 form ends after the Crafting and Furnace settings.
fn recipe_packet(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    for _ in 0..4 {
        wrapper.passthrough(&BOOL)?;
    }
    for _ in 0..4 {
        wrapper.read(&BOOL)?;
    }
    Ok(())
}

/// The old client expresses multi-block changes as chunk coordinates and
/// absolute block Y values. 1.16.2 groups them by section and packs them into
/// a long. The block ids have already been mapped to the 1.16.2 registry;
/// this adjacent patch version shares that block-state numbering.
fn section_blocks_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let packed_section = wrapper.read(&I64T)?;
    let chunk_x = (packed_section >> 42) as i32;
    let chunk_y = ((packed_section << 44) >> 44) as i32;
    let chunk_z = ((packed_section << 22) >> 42) as i32;
    wrapper.read(&BOOL)?; // Ignore old light data.
    let count = wrapper.read(&VAR_INT)?.0;
    if !(0..=MAX_BLOCK_CHANGES).contains(&count) {
        return Err(TranslateError::Unsupported("section block change count"));
    }

    let mut out = Vec::with_capacity(5 + count as usize * 5);
    I32T.write(&mut out, &chunk_x)?;
    I32T.write(&mut out, &chunk_z)?;
    VAR_INT.write(&mut out, &VarInt(count))?;
    for _ in 0..count {
        let packed_change = wrapper.read(&crate::api::types::VAR_LONG)?.0 as u64;
        let local = (packed_change & 0x0fff) as u16;
        let block = (packed_change >> 12) as i32;
        let x = i32::from((local >> 8) & 0x0f);
        let z = i32::from((local >> 4) & 0x0f);
        let y = (chunk_y << 4) | i32::from(local & 0x0f);
        let short_position = ((x << 12) | (z << 8) | (y & 0xff)) as i16;
        I16T.write(&mut out, &short_position)?;
        VAR_INT.write(&mut out, &VarInt(block))?;
    }
    wrapper.replace_remaining(out);
    wrapper.set_packet(&clientbound::play::MULTI_BLOCK_CHANGE);
    Ok(())
}

/// Vanilla 1.16.1 caches skull profiles by UUID and has MC-68487. Via replaces
/// SkullOwner.Id with a stable value hash based on the first texture value.
fn block_entity_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I64T)?;
    wrapper.passthrough(&U8T)?;
    let Some(NbtTag::Compound(mut block_entity)) = wrapper.read(&NbtT::for_version(ctx.layout))?
    else {
        wrapper.passthrough_all();
        return Ok(());
    };
    if block_entity
        .get_string("id")
        .is_some_and(|id| id.strip_prefix("minecraft:").unwrap_or(id) == "skull")
    {
        if let Some(mut owner) = block_entity.get_compound("SkullOwner").cloned()
            && owner.get("Id").is_some()
            && let Some(textures) = owner
                .get_compound("Properties")
                .and_then(|properties| properties.get_list("textures"))
            && let Some(NbtTag::Compound(first)) = textures.first()
            && let Some(value) = first.get_string("Value")
        {
            owner.put(
                "Id",
                NbtTag::IntArray(vec![java_string_hash(value), 0, 0, 0]),
            );
            block_entity.put_compound("SkullOwner", owner);
        }
    }
    wrapper.write(
        &NbtT::for_version(ctx.layout),
        &Some(NbtTag::Compound(block_entity)),
    )?;
    Ok(())
}

fn java_string_hash(value: &str) -> i32 {
    value.encode_utf16().fold(0i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

/// Via maps Piglin Brutes to Piglins. Update the tracker after the shared
/// spawn/id pass has rewritten the wire entity id, so its metadata uses the
/// stand-in's 1.16.1 schema.
fn track_piglin_stand_in(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let mut read = wrapper.remaining();
    let entity_id = read.get_var_int()?.0;
    if connection.entity_tracker.entity_type(entity_id)
        == Some(pumpkin_data::entity::EntityType::PIGLIN_BRUTE.id)
    {
        let piglin_id = crate::api::MappingData::get()
            .composed(ctx.layout)
            .entities
            .map(u32::from(pumpkin_data::entity::EntityType::PIGLIN.id))
            .and_then(|id| i32::try_from(id).ok())
            .ok_or(TranslateError::Unsupported("piglin entity id"))?;
        let mut spawn = CSpawnEntity::read_packet_data(wrapper.remaining(), &ctx.layout)?;
        spawn.r#type = VarInt(piglin_id);
        let mut payload = Vec::with_capacity(wrapper.remaining().len());
        spawn.write_packet_data(&mut payload, &ctx.layout)?;
        wrapper.replace_remaining(payload);
        connection.entity_tracker.add_mapped(
            entity_id,
            pumpkin_data::entity::EntityType::PIGLIN_BRUTE.id,
            pumpkin_data::entity::EntityType::PIGLIN.id,
        );
    }
    wrapper.passthrough_all();
    Ok(())
}

fn piglin_metadata(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let entity_id = wrapper.passthrough(&VAR_INT)?.0;
    let list = crate::api::entity_data::EntityDataListT::for_version(ctx.layout);
    let mut entries = wrapper.read(&list)?;
    let piglins = [
        pumpkin_data::entity::EntityType::PIGLIN.id,
        pumpkin_data::entity::EntityType::PIGLIN_BRUTE.id,
    ];
    if connection
        .entity_tracker
        .entity_type(entity_id)
        .is_some_and(|kind| piglins.contains(&kind))
    {
        for entry in &mut entries {
            entry.index = match entry.index {
                15 => 16,
                16 => 15,
                index => index,
            };
        }
    }
    wrapper.write(&list, &entries)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::text::TextComponent;

    #[test]
    fn the_step_has_exact_1_16_2_to_1_16_1_boundaries() {
        let step = Protocol1_16_2To1_16_1.step();
        assert_eq!(step.from, V::V_1_16_2);
        assert_eq!(step.to, V::V_1_16_1);
    }

    #[test]
    fn native_1_16_1_layouts_do_not_skip_the_step_handlers() {
        let mut registry = Registry::default();
        Protocol1_16_2To1_16_1.register(&mut registry);
        let step = Protocol1_16_2To1_16_1.step();
        let layout = V::V_1_16_1;

        for packet in [
            &clientbound::play::COMMANDS,
            &clientbound::play::UNLOCK_RECIPES,
            &clientbound::play::SECTION_BLOCKS_UPDATE,
            &clientbound::play::BLOCK_ENTITY_DATA,
            &clientbound::play::ADD_ENTITY,
            &clientbound::play::SET_ENTITY_DATA,
        ] {
            let entry = registry.clientbound_handler(packet).unwrap();
            assert!(
                !entry.layout,
                "{packet:?} must run on the native 1.16.1 layout"
            );
            assert!(entry.runs(step.from, layout), "{packet:?}");
        }

        let read_floor = crate::pipeline::core_layout::core_read_floor(
            &serverbound::play::RECIPE_BOOK_DATA,
            layout,
        );
        assert_eq!(read_floor, layout);
        let entry = registry
            .serverbound_handler(&serverbound::play::RECIPE_BOOK_DATA)
            .unwrap();
        assert!(!entry.layout, "legacy recipe book updates are read as sent");
        assert!(entry.runs(step.from, read_floor));

        let chat = registry
            .clientbound_handler(&clientbound::play::CHAT)
            .unwrap();
        assert!(chat.runs(
            step.from,
            crate::pipeline::core_layout::core_layout_floor(&clientbound::play::CHAT).max(layout)
        ));
    }

    #[test]
    fn native_login_and_respawn_layouts_skip_the_1_16_2_converters() {
        let step = Protocol1_16_2To1_16_1.step();
        let mut registry = Registry::default();
        Protocol1_16_2To1_16_1.register(&mut registry);
        for packet in [&clientbound::play::LOGIN, &clientbound::play::RESPAWN] {
            let floor = crate::pipeline::core_layout::core_layout_floor(packet);
            let layout = floor.max(step.to);
            assert_eq!(layout, step.to, "Pumpkin writes this packet for 1.16.1");
            let handler = registry.clientbound_handler(packet).unwrap();
            assert!(handler.layout);
            assert!(!handler.runs(step.from, layout));
        }
    }

    #[test]
    fn only_the_vanilla_dimension_effects_names_survive_login() {
        for (name, expected) in [
            ("minecraft:overworld", "minecraft:overworld"),
            ("minecraft:the_nether", "minecraft:the_nether"),
            ("minecraft:the_end", "minecraft:the_end"),
            ("example:moon", "minecraft:overworld"),
        ] {
            let mut compound = pumpkin_nbt::compound::NbtCompound::new();
            compound.put_string("effects", name.to_owned());
            assert_eq!(
                legacy_dimension(&Some(NbtTag::Compound(compound))).as_ref(),
                expected
            );
        }
        assert_eq!(legacy_dimension(&None).as_ref(), "minecraft:overworld");
    }

    #[test]
    fn legacy_dimension_codec_contains_the_four_via_default_entries() {
        let NbtTag::Compound(codec) = legacy_dimension_registry() else {
            panic!("expected legacy dimension codec compound");
        };
        let names = codec
            .get_list("dimension")
            .unwrap()
            .iter()
            .map(|entry| match entry {
                NbtTag::Compound(compound) => compound.get_string("name").unwrap(),
                _ => panic!("dimension registry entry must be a compound"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "minecraft:overworld",
                "minecraft:overworld_caves",
                "minecraft:the_nether",
                "minecraft:the_end"
            ]
        );
    }

    #[test]
    fn login_folds_hardcore_into_game_mode_and_caps_max_players() {
        let mut payload = Vec::new();
        I32T.write(&mut payload, &42).unwrap();
        BOOL.write(&mut payload, &true).unwrap();
        U8T.write(&mut payload, &1).unwrap();
        U8T.write(&mut payload, &0).unwrap();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap();
        STRING
            .write(&mut payload, &"minecraft:overworld".into())
            .unwrap();
        let mut source_codec = NbtCompound::new();
        source_codec.put_string("old", "source".to_owned());
        NbtT::for_version(V::V_1_16_2)
            .write(&mut payload, &Some(NbtTag::Compound(source_codec)))
            .unwrap();
        let mut dimension = NbtCompound::new();
        dimension.put_string("effects", "example:moon".to_owned());
        NbtT::for_version(V::V_1_16_2)
            .write(&mut payload, &Some(NbtTag::Compound(dimension)))
            .unwrap();
        STRING
            .write(&mut payload, &"minecraft:overworld".into())
            .unwrap();
        I64T.write(&mut payload, &123).unwrap();
        VAR_INT.write(&mut payload, &VarInt(300)).unwrap();
        VAR_INT.write(&mut payload, &VarInt(12)).unwrap();
        for flag in [true, false, false, true] {
            BOOL.write(&mut payload, &flag).unwrap();
        }

        let mut wrapper = PacketWrapper::new(&clientbound::play::LOGIN, &payload);
        let mut connection = UserConnection::new(0, V::V_1_16_1);
        login(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(V::V_1_16_2),
                layout: V::V_1_16_2,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        let mut read = translated.payload.as_slice();
        assert_eq!(I32T.read(&mut read).unwrap(), 42);
        assert_eq!(U8T.read(&mut read).unwrap(), 9);
        assert_eq!(U8T.read(&mut read).unwrap(), 0);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(
            STRING.read(&mut read).unwrap().as_ref(),
            "minecraft:overworld"
        );
        let Some(NbtTag::Compound(codec)) = NbtT::for_version(V::V_1_16_1).read(&mut read).unwrap()
        else {
            panic!("expected legacy dimension codec");
        };
        assert!(codec.get_list("dimension").is_some());
        assert_eq!(
            STRING.read(&mut read).unwrap().as_ref(),
            "minecraft:overworld"
        );
        assert_eq!(
            STRING.read(&mut read).unwrap().as_ref(),
            "minecraft:overworld"
        );
        assert_eq!(I64T.read(&mut read).unwrap(), 123);
        assert_eq!(U8T.read(&mut read).unwrap(), u8::MAX);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 12);
        assert_eq!(BOOL.read(&mut read).unwrap(), true);
        assert_eq!(BOOL.read(&mut read).unwrap(), false);
        assert_eq!(BOOL.read(&mut read).unwrap(), false);
        assert_eq!(BOOL.read(&mut read).unwrap(), true);
        assert!(read.is_empty());
    }

    #[test]
    fn an_action_bar_chat_message_becomes_title_action_two() {
        let version = V::V_1_16_1;
        let message = TextComponent::text("notice");
        let text = TextComponentT::for_version(version);
        let mut payload = Vec::new();
        text.write(&mut payload, &message).unwrap();
        U8T.write(&mut payload, &2).unwrap();

        let mut wrapper = PacketWrapper::new(&clientbound::play::CHAT, &payload);
        let mut connection = UserConnection::new(0, version);
        chat_message(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(V::V_1_16_2),
                layout: V::V_1_16_2,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            translated.packet.to_id(version),
            clientbound::play::TITLE.to_id(version)
        );
        let mut read = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2);
        assert_eq!(text.read(&mut read).unwrap(), message);
        assert!(read.is_empty());
    }

    #[test]
    fn the_angle_command_argument_becomes_a_single_word_string() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(2)).unwrap();
        U8.write(&mut payload, &0).unwrap();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap();
        U8.write(&mut payload, &2).unwrap();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap();
        STRING.write(&mut payload, &"rotation".into()).unwrap();
        STRING
            .write(&mut payload, &"minecraft:angle".into())
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap();

        let mut wrapper = PacketWrapper::new(&clientbound::play::COMMANDS, &payload);
        let mut connection = UserConnection::new(0, V::V_1_16_1);
        commands(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(V::V_1_16_2),
                layout: V::V_1_16_2,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        let mut read = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2);
        read.get_u8().unwrap();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        read.get_u8().unwrap();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0);
        assert_eq!(STRING.read(&mut read).unwrap().as_ref(), "rotation");
        assert_eq!(STRING.read(&mut read).unwrap().as_ref(), "brigadier:string");
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0);
        assert!(read.is_empty());
    }

    #[test]
    fn a_shown_recipe_changes_to_the_split_seen_recipe_packet() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap();
        STRING
            .write(&mut payload, &"minecraft:bread".into())
            .unwrap();
        let mut wrapper = PacketWrapper::new(&serverbound::play::RECIPE_BOOK_DATA, &payload);
        let mut connection = UserConnection::new(0, V::V_1_16_1);
        recipe_book_update(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(V::V_1_16_2),
                layout: V::V_1_16_2,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            translated.packet.to_id(V::V_1_16_2),
            serverbound::play::RECIPE_BOOK_SEEN_RECIPE.to_id(V::V_1_16_2)
        );
        assert_eq!(translated.payload, payload[1..]);
    }

    #[test]
    fn skull_texture_value_uses_java_utf16_hashing() {
        assert_eq!(java_string_hash("abc"), 96_354);
        assert_eq!(java_string_hash("💜"), 1_772_543);
    }

    #[test]
    fn legacy_recipe_settings_encode_each_category_and_preserve_both_flags() {
        for (recipe_type, open, filter) in [(0, true, false), (1, false, true), (2, true, true)] {
            let payload = recipe_setting_payload(recipe_type, open, filter).unwrap();
            let mut read = payload.as_slice();
            assert_eq!(VAR_INT.read(&mut read).unwrap().0, recipe_type);
            assert_eq!(BOOL.read(&mut read).unwrap(), open);
            assert_eq!(BOOL.read(&mut read).unwrap(), filter);
            assert!(read.is_empty());
        }
    }

    #[test]
    fn a_skull_block_entity_gets_a_texture_derived_profile_id() {
        let mut texture = pumpkin_nbt::compound::NbtCompound::new();
        texture.put_string("Value", "abc".to_owned());
        let mut properties = pumpkin_nbt::compound::NbtCompound::new();
        properties.put_list("textures", vec![NbtTag::Compound(texture)]);
        let mut owner = pumpkin_nbt::compound::NbtCompound::new();
        owner.put_string("Id", "original".to_owned());
        owner.put_compound("Properties", properties);
        let mut skull = pumpkin_nbt::compound::NbtCompound::new();
        skull.put_string("id", "minecraft:skull".to_owned());
        skull.put_compound("SkullOwner", owner);

        let mut payload = Vec::new();
        I64T.write(&mut payload, &0).unwrap();
        U8T.write(&mut payload, &3).unwrap();
        NbtT::for_version(V::V_1_16_2)
            .write(&mut payload, &Some(NbtTag::Compound(skull)))
            .unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::BLOCK_ENTITY_DATA, &payload);
        let mut connection = UserConnection::new(0, V::V_1_16_1);
        block_entity_data(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(V::V_1_16_2),
                layout: V::V_1_16_2,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        let mut read = translated.payload.as_slice();
        I64T.read(&mut read).unwrap();
        U8T.read(&mut read).unwrap();
        let Some(NbtTag::Compound(skull)) = NbtT::for_version(V::V_1_16_2).read(&mut read).unwrap()
        else {
            panic!("expected skull compound");
        };
        let owner = skull.get_compound("SkullOwner").unwrap();
        assert_eq!(
            owner.get("Id"),
            Some(&NbtTag::IntArray(vec![96_354, 0, 0, 0]))
        );
        assert!(read.is_empty());
    }

    #[test]
    fn piglin_metadata_indices_are_swapped_without_touching_others() {
        let mut indices = [14, 15, 16, 17];
        for index in &mut indices {
            *index = match *index {
                15 => 16,
                16 => 15,
                index => index,
            };
        }
        assert_eq!(indices, [14, 16, 15, 17]);
    }

    #[test]
    fn a_piglin_brute_spawn_uses_the_piglin_wire_type_and_metadata_tracker() {
        use pumpkin_util::math::vector3::Vector3;

        let version = V::V_1_16_2;
        let brute_id = crate::api::MappingData::get()
            .composed(version)
            .entities
            .map(u32::from(pumpkin_data::entity::EntityType::PIGLIN_BRUTE.id))
            .unwrap();
        let piglin_id = crate::api::MappingData::get()
            .composed(version)
            .entities
            .map(u32::from(pumpkin_data::entity::EntityType::PIGLIN.id))
            .unwrap();
        let spawn = CSpawnEntity::new(
            VarInt(7),
            uuid::Uuid::from_u128(1),
            VarInt(i32::try_from(brute_id).unwrap()),
            Vector3::new(1.0, 2.0, 3.0),
            0.0,
            0.0,
            0.0,
            VarInt(0),
            Vector3::new(0.0, 0.0, 0.0),
        );
        let mut payload = Vec::new();
        spawn.write_packet_data(&mut payload, &version).unwrap();
        let mut connection = UserConnection::new(0, V::V_1_16_1);
        connection.entity_tracker.add_mapped(
            7,
            pumpkin_data::entity::EntityType::PIGLIN_BRUTE.id,
            pumpkin_data::entity::EntityType::PIGLIN_BRUTE.id,
        );
        let mut wrapper = PacketWrapper::new(&clientbound::play::ADD_ENTITY, &payload);
        track_piglin_stand_in(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(version),
                layout: version,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        let spawn = CSpawnEntity::read_packet_data(&translated.payload, &version).unwrap();
        assert_eq!(spawn.r#type.0, i32::try_from(piglin_id).unwrap());
        assert_eq!(
            connection.entity_tracker.client_entity_type(7),
            Some(pumpkin_data::entity::EntityType::PIGLIN.id)
        );
    }

    #[test]
    fn legacy_section_updates_use_chunk_coordinates_and_absolute_y() {
        let chunk_x = 3i32;
        let chunk_y = 2i32;
        let chunk_z = -4i32;
        let position = ((i64::from(chunk_x) & 0x3F_FFFF) << 42)
            | ((i64::from(chunk_z) & 0x3F_FFFF) << 20)
            | i64::from(chunk_y & 0xF_FFFF);
        let local = 0xA35u64; // x=10, z=3, y=5
        let packed_block = (7u64 << 12) | local;
        let mut payload = Vec::new();
        I64T.write(&mut payload, &position).unwrap();
        BOOL.write(&mut payload, &true).unwrap();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap();
        crate::api::types::VAR_LONG
            .write(&mut payload, &VarLong(packed_block as i64))
            .unwrap();

        let mut wrapper = PacketWrapper::new(&clientbound::play::SECTION_BLOCKS_UPDATE, &payload);
        let mut connection = UserConnection::new(0, V::V_1_16_1);
        section_blocks_update(
            &mut wrapper,
            &mut connection,
            &Ctx {
                step: Protocol1_16_2To1_16_1.step(),
                mappings: crate::api::MappingData::get().step(V::V_1_16_2),
                layout: V::V_1_16_2,
            },
        )
        .unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            translated.packet.to_id(V::V_1_16_1),
            clientbound::play::MULTI_BLOCK_CHANGE.to_id(V::V_1_16_1)
        );
        let mut read = translated.payload.as_slice();
        assert_eq!(read.get_i32_be().unwrap(), chunk_x);
        assert_eq!(read.get_i32_be().unwrap(), chunk_z);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(
            I16T.read(&mut read).unwrap() as u16,
            (10 << 12) | (3 << 8) | 37
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 7);
        assert!(read.is_empty());
    }
}
