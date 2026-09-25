//! ViaBackwards 1.16 to 1.15.2 compatibility rules.
//!
//! The registry tables for this boundary are vendored in
//! `assets/viabackwards/data/mappings-1.16to1.15.nbt`; root-chain registration
//! is kept in `protocol::mod` so this slice can be integrated independently.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{attribute_id_to_legacy_name, attribute_name_to_id};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::TextComponentBase;
use pumpkin_util::text::TextContent;
use pumpkin_util::text::click::ClickEvent;
use pumpkin_util::text::color::{Color, NamedColor, RGBColor};
use pumpkin_util::text::hover::HoverEvent;
use pumpkin_util::version::JavaMinecraftVersion as V;
use uuid::Uuid;

use crate::api::types::{
    BOOL, BYTE_ARRAY, F32T, F64T, I8T, I32T, NbtT, OptionalT, STRING, TextComponentT, U8T, UUID,
    VAR_INT, WireType,
};
use crate::api::{
    Ctx, MappingData, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection,
};
use crate::packet::mappings::{clientbound, serverbound};

const MAP_COLOR_REWRITES: &[(u8, u8)] = &[
    (208, 113),
    (209, 114),
    (210, 114),
    (211, 112),
    (212, 152),
    (213, 83),
    (214, 83),
    (215, 155),
    (216, 143),
    (217, 115),
    (218, 115),
    (219, 143),
    (220, 127),
    (221, 127),
    (222, 127),
    (223, 95),
    (224, 127),
    (225, 127),
    (226, 124),
    (227, 95),
    (228, 187),
    (229, 155),
    (230, 184),
    (231, 187),
    (232, 127),
    (233, 124),
    (234, 125),
    (235, 127),
];

#[derive(Default)]
struct ProtocolStorage {
    sneaking: bool,
    world_name: Option<Box<str>>,
    dimension: Option<i32>,
    player_attributes: HashMap<String, Vec<u8>>,
}

pub struct Protocol1_16To1_15_2;

impl Protocol for Protocol1_16To1_15_2 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_16,
            to: V::V_1_15_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::CHAT, chat);
        reg.clientbound(&clientbound::play::SYSTEM_CHAT, system_chat);
        reg.clientbound(&clientbound::play::OPEN_SCREEN, open_screen);
        reg.clientbound(&clientbound::status::STATUS_RESPONSE, status_response);
        reg.clientbound(&clientbound::login::LOGIN_FINISHED, login_finished);
        reg.clientbound(&clientbound::play::PLAYER_INFO, player_info);
        reg.clientbound(&clientbound::play::UPDATE_ATTRIBUTES, update_attributes);
        reg.clientbound(&clientbound::play::MAP_ITEM_DATA, map_item_data);
        reg.clientbound(&clientbound::play::BLOCK_ENTITY_DATA, block_entity_data);
        reg.clientbound(&clientbound::play::COMMANDS, commands);

        reg.serverbound(&serverbound::play::PLAYER_COMMAND, player_command);
        reg.serverbound(&serverbound::play::INTERACT, interact);
        reg.serverbound(&serverbound::play::PLAYER_ABILITIES, player_abilities);
        reg.cancel_serverbound(&serverbound::play::SET_JIGSAW_BLOCK);
    }
}

fn storage(connection: &mut UserConnection) -> &mut ProtocolStorage {
    if connection.get::<ProtocolStorage>().is_none() {
        connection.put(ProtocolStorage::default());
    }
    connection
        .get_mut::<ProtocolStorage>()
        .expect("protocol storage was inserted")
}

/// Records the server world identity before a lower-version respawn is
/// reduced to the legacy numeric dimension and level type fields. The join
/// codec integration should call this while it still has the 1.16 world name.
pub fn update_world_identity(
    connection: &mut UserConnection,
    world_name: &str,
    dimension: i32,
) -> bool {
    let state = storage(connection);
    let world_changed = state.dimension == Some(dimension)
        && state
            .world_name
            .as_deref()
            .is_some_and(|old| old != world_name);
    state.world_name = Some(world_name.into());
    state.dimension = Some(dimension);
    world_changed
}

/// The latest mapped player attributes are retained so a respawn path that
/// carries the 1.16 keep-data bit can re-send them after its legacy respawn.
pub fn saved_player_attributes(connection: &mut UserConnection) -> Option<Vec<u8>> {
    let entity_id = connection.entity_tracker.client_entity_id?;
    let attributes = &storage(connection).player_attributes;
    if attributes.is_empty() {
        return None;
    }
    let count = i32::try_from(attributes.len()).ok()?;
    let mut payload = Vec::new();
    VAR_INT.write(&mut payload, &VarInt(entity_id)).ok()?;
    I32T.write(&mut payload, &count).ok()?;
    for record in attributes.values() {
        payload.extend_from_slice(record);
    }
    Some(payload)
}

fn translate_component(component: &mut TextComponent) {
    translate_base(&mut component.0);
}

fn translate_base(base: &mut TextComponentBase) {
    match base.content.as_mut() {
        TextContent::Translate {
            translate, with, ..
        } => {
            if let Some(mapped) = translations().get(translate.as_ref()) {
                *translate = Cow::Owned(mapped.clone());
            }
            for child in with {
                translate_base(child);
            }
        }
        TextContent::Custom { key, with, .. } => {
            if let Some(mapped) = translations().get(key.as_ref()) {
                *key = Cow::Owned(mapped.clone());
            }
            for child in with {
                translate_base(child);
            }
        }
        _ => {}
    }

    if let Some(Color::Rgb(rgb)) = base.style.color {
        base.style.color = Some(Color::Named(nearest_chat_color(rgb)));
    }
    if let Some(ClickEvent::CopyToClipboard { value }) = base.style.click_event.as_ref() {
        base.style.click_event = Some(ClickEvent::SuggestCommand {
            command: Cow::Owned(value.to_string()),
        });
    }
    if let Some(hover) = base.style.hover_event.as_mut() {
        match hover {
            HoverEvent::ShowText { value } => value.iter_mut().for_each(translate_base),
            HoverEvent::ShowEntity {
                name: Some(name), ..
            } => {
                name.iter_mut().for_each(translate_base);
            }
            HoverEvent::ShowItem { .. } | HoverEvent::ShowEntity { name: None, .. } => {}
        }
    }
    base.extra.iter_mut().for_each(translate_base);
}

fn translations() -> &'static HashMap<String, String> {
    static TRANSLATIONS: OnceLock<HashMap<String, String>> = OnceLock::new();
    TRANSLATIONS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../assets/viabackwards/data/translations-1.16.json"
        ))
        .expect("vendored 1.16 translation mappings are valid JSON")
    })
}

fn nearest_chat_color(rgb: RGBColor) -> NamedColor {
    const COLORS: [(NamedColor, u32); 16] = [
        (NamedColor::Black, 0x000000),
        (NamedColor::DarkBlue, 0x0000aa),
        (NamedColor::DarkGreen, 0x00aa00),
        (NamedColor::DarkAqua, 0x00aaaa),
        (NamedColor::DarkRed, 0xaa0000),
        (NamedColor::DarkPurple, 0xaa00aa),
        (NamedColor::Gold, 0xffaa00),
        (NamedColor::Gray, 0xaaaaaa),
        (NamedColor::DarkGray, 0x555555),
        (NamedColor::Blue, 0x5555ff),
        (NamedColor::Green, 0x55ff55),
        (NamedColor::Aqua, 0x55ffff),
        (NamedColor::Red, 0xff5555),
        (NamedColor::LightPurple, 0xff55ff),
        (NamedColor::Yellow, 0xffff55),
        (NamedColor::White, 0xffffff),
    ];
    let red = i32::from(rgb.red);
    let green = i32::from(rgb.green);
    let blue = i32::from(rgb.blue);
    let mut best = NamedColor::Black;
    let mut best_distance = i64::MAX;
    for (color, value) in COLORS {
        let r = ((value >> 16) & 0xff) as i32;
        let g = ((value >> 8) & 0xff) as i32;
        let b = (value & 0xff) as i32;
        let r_average = (r + red) / 2;
        let r_diff = r - red;
        let g_diff = g - green;
        let b_diff = b - blue;
        let distance = i64::from((2 + (r_average >> 8)) * r_diff * r_diff)
            + i64::from(4 * g_diff * g_diff)
            + i64::from((2 + ((255 - r_average) >> 8)) * b_diff * b_diff);
        if distance < best_distance {
            best = color;
            best_distance = distance;
        }
    }
    best
}

fn rewrite_text(wrapper: &mut PacketWrapper, from: V, to: V) -> Result<(), TranslateError> {
    let source = TextComponentT::for_version(from);
    let target = TextComponentT::for_version(to);
    let mut component = wrapper.read(&source)?;
    translate_component(&mut component);
    wrapper.write(&target, &component)
}

fn chat(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    rewrite_text(wrapper, ctx.step.from, ctx.step.to)?;
    wrapper.passthrough(&U8T)?;
    wrapper.passthrough(&UUID)?;
    wrapper.passthrough_all();
    Ok(())
}

fn system_chat(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let source = TextComponentT::for_version(ctx.step.from);
    let output = TextComponentT::for_version(ctx.step.to);
    let mut component = wrapper.read(&source)?;
    translate_component(&mut component);
    wrapper.write(&output, &component)?;
    let overlay = wrapper.read(&BOOL)?;
    wrapper.write(&U8T, &if overlay { 2 } else { 1 })?;
    wrapper.write(&UUID, &Uuid::nil())?;
    wrapper.passthrough_all();
    wrapper.set_packet(&clientbound::play::CHAT);
    Ok(())
}

fn open_screen(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let menu = wrapper.passthrough(&VAR_INT)?.0;
    let menu = if menu == 20 {
        7
    } else if menu > 20 {
        menu - 1
    } else {
        menu
    };
    wrapper.write(&VAR_INT, &VarInt(menu))?;
    rewrite_text(wrapper, ctx.step.from, ctx.step.to)?;
    wrapper.passthrough_all();
    Ok(())
}

fn status_response(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let raw = wrapper.read(&STRING)?;
    let mut status: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| TranslateError::Unsupported("status JSON"))?;
    if let Some(description) = status.get_mut("description") {
        rewrite_json_component(description);
    }
    wrapper.write(&STRING, &status.to_string().into_boxed_str())?;
    wrapper.passthrough_all();
    Ok(())
}

fn rewrite_json_component(value: &mut serde_json::Value) {
    if let Ok(mut component) = serde_json::from_value::<TextComponent>(value.clone()) {
        translate_component(&mut component);
        if let Ok(json) = serde_json::from_str(&component.to_json_for_version(&V::V_1_15_2)) {
            *value = json;
        }
    }
}

fn login_finished(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let uuid = wrapper.read(&UUID)?;
    wrapper.write(&STRING, &uuid.to_string().into_boxed_str())?;
    wrapper.passthrough_all();
    Ok(())
}

fn player_info(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let action = wrapper.passthrough(&VAR_INT)?.0;
    let count = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("player-info count"));
    }
    let text = TextComponentT::for_version(ctx.step.from);
    let output_text = TextComponentT::for_version(ctx.step.to);
    for _ in 0..count {
        wrapper.passthrough(&UUID)?;
        match action {
            0 => {
                wrapper.passthrough(&STRING)?;
                let properties = wrapper.passthrough(&VAR_INT)?.0;
                if !(0..=1024).contains(&properties) {
                    return Err(TranslateError::Unsupported("profile-property count"));
                }
                for _ in 0..properties {
                    wrapper.passthrough(&STRING)?;
                    wrapper.passthrough(&STRING)?;
                    wrapper.passthrough(&OptionalT(STRING))?;
                }
                wrapper.passthrough(&VAR_INT)?;
                wrapper.passthrough(&VAR_INT)?;
                let mut component = wrapper.read(&OptionalT(text))?;
                if let Some(component) = component.as_mut() {
                    translate_component(component);
                }
                wrapper.write(&OptionalT(output_text), &component)?;
            }
            1 | 2 => {
                wrapper.passthrough(&VAR_INT)?;
            }
            3 => {
                let mut component = wrapper.read(&OptionalT(text))?;
                if let Some(component) = component.as_mut() {
                    translate_component(component);
                }
                wrapper.write(&OptionalT(output_text), &component)?;
            }
            4 => {}
            _ => return Err(TranslateError::Unsupported("player-info action")),
        }
    }
    wrapper.passthrough_all();
    Ok(())
}

fn update_attributes(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let entity_id = wrapper.passthrough(&VAR_INT)?.0;
    let count = wrapper.passthrough(&I32T)?;
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("attribute count"));
    }
    let ids = MappingData::get().composed(ctx.step.to);
    let is_player = connection.entity_tracker.client_entity_id == Some(entity_id);
    let mut attributes = Vec::new();
    I32T.write(&mut attributes, &count)?;
    for _ in 0..count {
        let source_name = wrapper.read(&STRING)?;
        let mapped_name = attribute_name_to_id(&source_name)
            .and_then(|id| ids.attributes.map(u32::from(id)))
            .and_then(|id| u8::try_from(id).ok())
            .map(attribute_id_to_legacy_name)
            .ok_or(TranslateError::Unsupported("attribute mapping"))?;
        let mut record = Vec::new();
        STRING.write(&mut record, &mapped_name.to_owned().into_boxed_str())?;
        let value = wrapper.passthrough(&F64T)?;
        F64T.write(&mut record, &value)?;
        let modifiers = wrapper.passthrough(&VAR_INT)?.0;
        if !(0..=1024).contains(&modifiers) {
            return Err(TranslateError::Unsupported("attribute modifier count"));
        }
        VAR_INT.write(&mut record, &VarInt(modifiers))?;
        for _ in 0..modifiers {
            let uuid = wrapper.passthrough(&UUID)?;
            UUID.write(&mut record, &uuid)?;
            let amount = wrapper.passthrough(&F64T)?;
            F64T.write(&mut record, &amount)?;
            let operation = wrapper.passthrough(&I8T)?;
            I8T.write(&mut record, &operation)?;
        }
        if is_player {
            storage(connection)
                .player_attributes
                .insert(mapped_name.to_owned(), record.clone());
        }
        attributes.extend_from_slice(&record);
    }
    wrapper.write_bytes(&attributes);
    Ok(())
}

fn map_item_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&I8T)?;
    wrapper.passthrough(&BOOL)?; // tracking position
    wrapper.passthrough(&BOOL)?; // 1.16 locked flag is absent in 1.15.2
    let count = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("map decoration count"));
    }
    let text = TextComponentT::for_version(ctx.step.from);
    let output_text = TextComponentT::for_version(ctx.step.to);
    for _ in 0..count {
        wrapper.passthrough(&VAR_INT)?;
        wrapper.passthrough(&I8T)?;
        wrapper.passthrough(&I8T)?;
        wrapper.passthrough(&U8T)?;
        let mut name = wrapper.read(&OptionalT(text))?;
        if let Some(component) = name.as_mut() {
            translate_component(component);
        }
        wrapper.write(&OptionalT(output_text), &name)?;
    }
    let columns = wrapper.passthrough(&U8T)?;
    if columns != 0 {
        wrapper.passthrough(&U8T)?;
        wrapper.passthrough(&U8T)?;
        wrapper.passthrough(&U8T)?;
        let mut colors = wrapper.read(&BYTE_ARRAY)?;
        for color in &mut colors {
            if let Some((_, mapped)) = MAP_COLOR_REWRITES
                .iter()
                .find(|(source, _)| *source == *color)
            {
                *color = *mapped;
            }
        }
        wrapper.write(&BYTE_ARRAY, &colors)?;
    }
    wrapper.passthrough_all();
    Ok(())
}

fn rewrite_block_entity(tag: &mut NbtTag) {
    let NbtTag::Compound(root) = tag else { return };
    let block_entity_id = root.get_string("id").unwrap_or("");
    match block_entity_id
        .strip_prefix("minecraft:")
        .unwrap_or(block_entity_id)
    {
        "conduit" => {
            if let Some(NbtTag::IntArray(parts)) = root.child_tags.remove("Target") {
                if parts.len() == 4 {
                    let mut bytes = [0u8; 16];
                    for (index, part) in parts.iter().enumerate() {
                        bytes[index * 4..index * 4 + 4].copy_from_slice(&part.to_be_bytes());
                    }
                    root.put_string("target_uuid", Uuid::from_bytes(bytes).to_string());
                }
            }
        }
        "skull" => {
            let Some(NbtTag::Compound(mut owner)) = root.child_tags.remove("SkullOwner") else {
                return;
            };
            if let Some(NbtTag::IntArray(parts)) = owner.child_tags.get("Id") {
                if parts.len() == 4 {
                    let mut bytes = [0u8; 16];
                    for (index, part) in parts.iter().enumerate() {
                        bytes[index * 4..index * 4 + 4].copy_from_slice(&part.to_be_bytes());
                    }
                    owner.put_string("Id", Uuid::from_bytes(bytes).to_string());
                }
            }
            root.put_compound("Owner", owner);
        }
        _ => {}
    }
}

fn block_entity_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I64T)?;
    wrapper.passthrough(&U8T)?;
    let nbt = NbtT::for_version(ctx.step.from);
    let mut tag = wrapper.read(&nbt)?;
    if let Some(tag) = tag.as_mut() {
        rewrite_block_entity(tag);
    }
    wrapper.write(&NbtT::for_version(ctx.step.to), &tag)?;
    wrapper.passthrough_all();
    Ok(())
}

fn skip_command_properties(
    wrapper: &mut PacketWrapper,
    parser: &str,
) -> Result<(), TranslateError> {
    match parser {
        "brigadier:bool" => {}
        "brigadier:float" => skip_number(wrapper, &F32T)?,
        "brigadier:double" => skip_number(wrapper, &F64T)?,
        "brigadier:integer" => skip_number(wrapper, &I32T)?,
        "brigadier:long" => skip_number(wrapper, &crate::api::I64T)?,
        "brigadier:string" => {
            wrapper.passthrough(&VAR_INT)?;
        }
        "minecraft:entity" | "minecraft:score_holder" => {
            wrapper.passthrough(&U8T)?;
        }
        "minecraft:time" => {}
        "minecraft:uuid"
        | "minecraft:game_profile"
        | "minecraft:nbt_compound_tag"
        | "minecraft:nbt_tag"
        | "minecraft:nbt_path"
        | "minecraft:objective"
        | "minecraft:scoreboard_slot"
        | "minecraft:swizzle"
        | "minecraft:team"
        | "minecraft:message"
        | "minecraft:component"
        | "minecraft:color"
        | "minecraft:angle"
        | "minecraft:rotation"
        | "minecraft:block_pos"
        | "minecraft:column_pos"
        | "minecraft:vec3"
        | "minecraft:vec2"
        | "minecraft:block_state"
        | "minecraft:block_predicate"
        | "minecraft:item_stack"
        | "minecraft:item_predicate"
        | "minecraft:entity_summon"
        | "minecraft:particle"
        | "minecraft:function"
        | "minecraft:resource_location"
        | "minecraft:resource"
        | "minecraft:resource_or_tag"
        | "minecraft:resource_key"
        | "minecraft:resource_or_tag_key" => {}
        _ => return Err(TranslateError::Unsupported("command parser properties")),
    }
    Ok(())
}

fn skip_number<T: WireType>(wrapper: &mut PacketWrapper, number: &T) -> Result<(), TranslateError> {
    let flags = wrapper.passthrough(&U8T)?;
    if flags & 1 != 0 {
        wrapper.passthrough(number)?;
    }
    if flags & 2 != 0 {
        wrapper.passthrough(number)?;
    }
    Ok(())
}

fn commands(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let count = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=65_536).contains(&count) {
        return Err(TranslateError::Unsupported("command node count"));
    }
    for _ in 0..count {
        let flags = wrapper.passthrough(&U8T)?;
        let children = wrapper.passthrough(&VAR_INT)?.0;
        if !(0..=65_536).contains(&children) {
            return Err(TranslateError::Unsupported("command child count"));
        }
        for _ in 0..children {
            wrapper.passthrough(&VAR_INT)?;
        }
        if flags & 8 != 0 {
            wrapper.passthrough(&VAR_INT)?;
        }
        let node_type = flags & 3;
        if node_type == 1 || node_type == 2 {
            wrapper.passthrough(&STRING)?;
        }
        if node_type == 2 {
            let parser = wrapper.read(&STRING)?;
            let mapped = if parser.as_ref() == "minecraft:uuid" {
                "minecraft:game_profile"
            } else {
                &parser
            };
            wrapper.write(&STRING, &mapped.to_owned().into_boxed_str())?;
            skip_command_properties(wrapper, &parser)?;
        }
        if flags & 16 != 0 {
            wrapper.passthrough(&STRING)?;
        }
    }
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough_all();
    Ok(())
}

fn player_command(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    match wrapper.passthrough(&VAR_INT)?.0 {
        0 => storage(connection).sneaking = true,
        1 => storage(connection).sneaking = false,
        _ => {}
    }
    wrapper.passthrough_all();
    Ok(())
}

fn interact(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let action = wrapper.passthrough(&VAR_INT)?.0;
    if action == 0 || action == 2 {
        if action == 2 {
            wrapper.passthrough(&F32T)?;
            wrapper.passthrough(&F32T)?;
            wrapper.passthrough(&F32T)?;
        }
        wrapper.passthrough(&VAR_INT)?;
    }
    wrapper.write(&BOOL, &storage(connection).sneaking)?;
    wrapper.passthrough_all();
    Ok(())
}

fn player_abilities(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let flags = wrapper.read(&U8T)? & 2;
    wrapper.write(&U8T, &flags)?;
    wrapper.read(&F32T)?;
    wrapper.read(&F32T)?;
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_nbt::NbtCompound;
    use pumpkin_protocol::ser::NetworkWriteExt;

    #[test]
    fn map_palette_entries_keep_their_brightness_bits() {
        let input = [208, 209, 235, 207, 1];
        let out: Vec<u8> = input
            .iter()
            .map(|color| {
                MAP_COLOR_REWRITES
                    .iter()
                    .find(|(source, _)| source == color)
                    .map_or(*color, |(_, target)| *target)
            })
            .collect();
        assert_eq!(out, [113, 114, 127, 207, 1]);
    }

    #[test]
    fn the_text_rewriter_downsamples_rgb_and_legacy_click_actions() {
        let mut component = TextComponent::text("go")
            .color(Color::Rgb(RGBColor::new(255, 85, 85)))
            .click_event(ClickEvent::CopyToClipboard {
                value: Cow::Borrowed("hello"),
            });
        translate_component(&mut component);
        assert_eq!(component.0.style.color, Some(Color::Named(NamedColor::Red)));
        assert!(matches!(
            component.0.style.click_event,
            Some(ClickEvent::SuggestCommand { .. })
        ));
    }

    #[test]
    fn new_translation_keys_map_to_the_1_15_names() {
        assert_eq!(
            translations()
                .get("attribute.name.generic.max_health")
                .map(String::as_str),
            Some("Max Health")
        );
    }

    #[test]
    fn a_system_message_becomes_a_legacy_chat_message() {
        let component = TextComponent::text("info");
        let mut payload = Vec::new();
        TextComponentT::for_version(V::V_1_16)
            .write(&mut payload, &component)
            .unwrap();
        payload.push(1); // overlay
        let mut wrapper = PacketWrapper::new(&clientbound::play::SYSTEM_CHAT, &payload);
        let mut connection = UserConnection::new(0, V::V_1_15_2);
        let ctx = Ctx {
            step: Protocol1_16To1_15_2.step(),
            mappings: MappingData::get().step(V::V_1_16),
            layout: V::V_26_3,
        };
        system_chat(&mut wrapper, &mut connection, &ctx).unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            translated.packet.to_id(V::V_1_15_2),
            clientbound::play::CHAT.to_id(V::V_1_15_2)
        );
        let mut read = translated.payload.as_slice();
        TextComponentT::for_version(V::V_1_15_2)
            .read(&mut read)
            .unwrap();
        assert_eq!(U8T.read(&mut read).unwrap(), 2);
        assert_eq!(UUID.read(&mut read).unwrap(), Uuid::nil());
        assert!(read.is_empty());
    }

    #[test]
    fn command_uuid_arguments_become_game_profile_arguments() {
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(2)).unwrap();
        payload.write_u8(0).unwrap();
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.write_u8(2).unwrap();
        payload.write_var_int(&VarInt(0)).unwrap();
        payload.write_string("profile").unwrap();
        payload.write_string("minecraft:uuid").unwrap();
        payload.write_var_int(&VarInt(0)).unwrap();

        let mut wrapper = PacketWrapper::new(&clientbound::play::COMMANDS, &payload);
        let mut connection = UserConnection::new(0, V::V_1_15_2);
        let ctx = Ctx {
            step: Protocol1_16To1_15_2.step(),
            mappings: MappingData::get().step(V::V_1_16),
            layout: V::V_26_3,
        };
        commands(&mut wrapper, &mut connection, &ctx).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;
        assert!(
            out.windows(b"minecraft:game_profile".len())
                .any(|slice| slice == b"minecraft:game_profile")
        );
        assert!(
            !out.windows(b"minecraft:uuid".len())
                .any(|slice| slice == b"minecraft:uuid")
        );
    }

    #[test]
    fn interaction_includes_the_last_sneak_state() {
        let mut connection = UserConnection::new(0, V::V_1_15_2);
        storage(&mut connection).sneaking = true;
        let mut wrapper = PacketWrapper::new(&serverbound::play::INTERACT, &[5, 0, 0]);
        let ctx = Ctx {
            step: Protocol1_16To1_15_2.step(),
            mappings: MappingData::get().step(V::V_1_16),
            layout: V::V_26_3,
        };
        interact(&mut wrapper, &mut connection, &ctx).unwrap();
        assert_eq!(wrapper.finish().unwrap().unwrap().payload, [5, 0, 0, 1]);
    }

    #[test]
    fn conduit_and_skull_uuid_fields_use_legacy_string_shapes() {
        let uuid = Uuid::from_u128(0x0102030405060708090a0b0c0d0e0f10);
        let words = uuid
            .as_bytes()
            .chunks_exact(4)
            .map(|chunk| i32::from_be_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        let mut conduit = NbtCompound::new();
        conduit.put_string("id", "minecraft:conduit".into());
        conduit.put("Target", NbtTag::IntArray(words.clone()));
        let mut tag = NbtTag::Compound(conduit);
        rewrite_block_entity(&mut tag);
        let NbtTag::Compound(conduit) = tag else {
            panic!("compound")
        };
        let expected_uuid = uuid.to_string();
        assert_eq!(
            conduit.get_string("target_uuid"),
            Some(expected_uuid.as_str())
        );

        let mut owner = NbtCompound::new();
        owner.put("Id", NbtTag::IntArray(words));
        let mut skull = NbtCompound::new();
        skull.put_string("id", "minecraft:skull".into());
        skull.put_compound("SkullOwner", owner);
        let mut tag = NbtTag::Compound(skull);
        rewrite_block_entity(&mut tag);
        let NbtTag::Compound(skull) = tag else {
            panic!("compound")
        };
        assert_eq!(
            skull
                .get_compound("Owner")
                .and_then(|owner| owner.get_string("Id")),
            Some(expected_uuid.as_str())
        );
    }
}
