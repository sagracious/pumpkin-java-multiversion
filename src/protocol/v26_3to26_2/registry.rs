use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::types::{BOOL, NbtT, VAR_INT, WireType};
use crate::api::{Ctx, PacketWrapper, Registry, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;

const REMOVED_REGISTRIES: &[&str] = &[
    "minecraft:decorated_pot_pattern",
    "decorated_pot_pattern",
    "minecraft:block_transformer",
    "block_transformer",
    "minecraft:worldgen/block_state_provider",
    "worldgen/block_state_provider",
];

pub(super) fn register(reg: &mut Registry) {
    reg.clientbound(&clientbound::config::REGISTRY_DATA, registry_data);
    reg.clientbound(&clientbound::config::UPDATE_TAGS, update_tags);
    reg.clientbound(&clientbound::play::UPDATE_TAGS, update_tags);
    reg.cancel_clientbound(&clientbound::play::ADD_TRANSIENT_BLOCK);
    reg.cancel_clientbound(&clientbound::play::POST_EFFECTS);
    reg.cancel_clientbound(&clientbound::config::POST_EFFECTS);
}

fn rename_registry_tag(tag_name: &str, registry: &str) -> String {
    let block_registry = registry == "minecraft:block" || registry == "block";
    if block_registry
        && (tag_name == "minecraft:convertible_to_mud" || tag_name == "convertible_to_mud")
    {
        return tag_name.replace("convertible_to_mud", "convertable_to_mud");
    }
    tag_name.to_string()
}

fn rewrite_tag_members(cursor: &mut &[u8], out: &mut Vec<u8>, registry: &str) -> Option<()> {
    let tags = cursor.get_var_int().ok()?.0;
    if !(0..=65536).contains(&tags) {
        return None;
    }
    out.write_var_int(&VarInt(tags)).ok()?;
    for _ in 0..tags {
        let name: String = cursor.get_str().ok()?.into();
        out.write_string(&rename_registry_tag(&name, registry))
            .ok()?;
        let count = cursor.get_var_int().ok()?.0;
        if !(0..=1_000_000).contains(&count) {
            return None;
        }
        out.write_var_int(&VarInt(count)).ok()?;
        for _ in 0..count {
            out.write_var_int(&cursor.get_var_int().ok()?).ok()?;
        }
    }
    Some(())
}

fn rewrite_tags(payload: &[u8], version: V) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let mut out = Vec::with_capacity(payload.len());
    if version < V::V_1_17 {
        // 1.16.x fixes the four registry groups by packet order; blocks are first.
        for registry in ["block", "item", "fluid", "entity_type"] {
            rewrite_tag_members(&mut cursor, &mut out, registry)?;
        }
    } else {
        let groups = cursor.get_var_int()?.0;
        if !(0..=256).contains(&groups) {
            return None;
        }
        out.write_var_int(&VarInt(groups)).ok()?;
        for _ in 0..groups {
            let registry: String = cursor.get_str().ok()?.into();
            out.write_string(&registry).ok()?;
            rewrite_tag_members(&mut cursor, &mut out, &registry)?;
        }
    }
    cursor.is_empty().then_some(out)
}

fn update_tags(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if let Some(out) = rewrite_tags(wrapper.remaining(), ctx.layout) {
        wrapper.replace_remaining(out);
    } else {
        wrapper.passthrough_all();
    }
    Ok(())
}

fn trim_asset_name(palette_id: &str) -> &str {
    let path = palette_id
        .rsplit_once(':')
        .map_or(palette_id, |(_, path)| path);
    path.strip_prefix("trim/").unwrap_or(path)
}

fn handle_environment_attributes(root: &mut NbtCompound) {
    let Some(NbtTag::Compound(attributes)) = root.child_tags.get_mut("attributes") else {
        return;
    };
    for value in attributes.child_tags.values_mut() {
        let NbtTag::Compound(attribute) = value else {
            continue;
        };
        if let Some(NbtTag::String(modifier)) = attribute.child_tags.get_mut("modifier")
            && (modifier.as_ref() == "append" || modifier.as_ref() == "overlay")
        {
            *modifier = "override".into();
        }
    }
}

fn update_full_block_state(state: &mut NbtCompound) {
    if let Some(name) = state.child_tags.remove("id") {
        state.put("Name", name);
    }
    if let Some(properties) = state.child_tags.remove("properties") {
        state.put("Properties", properties);
    }
}

fn update_block_state(parent: &mut NbtCompound, key: &str) {
    match parent.child_tags.remove(key) {
        Some(NbtTag::Compound(mut state)) => {
            update_full_block_state(&mut state);
            parent.put_compound(key, state);
        }
        Some(NbtTag::String(name)) => {
            let mut state = NbtCompound::new();
            state.put("Name", NbtTag::String(name));
            parent.put_compound(key, state);
        }
        Some(other) => parent.put(key, other),
        None => {}
    }
}

fn update_block_state_provider(tag: &mut NbtCompound) {
    let Some(NbtTag::String(provider_type)) = tag.child_tags.get("type") else {
        let mut state = tag.clone();
        update_full_block_state(&mut state);
        tag.put_compound("state", state);
        tag.put_string("type", "simple_state_provider".to_string());
        return;
    };
    let provider_type = provider_type.to_string();
    match provider_type
        .strip_prefix("minecraft:")
        .unwrap_or(&provider_type)
    {
        "simple_state_provider" | "rotated_block_provider" => update_block_state(tag, "state"),
        "weighted_state_provider" => {
            if let Some(NbtTag::List(entries)) = tag.child_tags.get_mut("entries") {
                for entry in entries {
                    if let NbtTag::Compound(entry) = entry {
                        update_block_state(entry, "data");
                    }
                }
            }
        }
        "noise_threshold_provider" => update_block_state(tag, "default_state"),
        _ => {}
    }
}

fn update_tag_key(tags: &mut [NbtTag]) {
    for tag in tags {
        let NbtTag::Compound(tag) = tag else {
            continue;
        };
        match tag.child_tags.get_mut("id") {
            Some(NbtTag::String(id)) => {
                if let Some(stripped) = id.strip_prefix('#') {
                    *id = stripped.to_string().into_boxed_str();
                }
            }
            _ => tag.put_string("id", "wool".to_string()),
        }
    }
}

fn update_match_block(term: &mut NbtCompound) {
    let blocks = term.child_tags.remove("blocks");
    if let Some(NbtTag::String(block)) = blocks
        && !block.starts_with('#')
    {
        term.put_string("condition", "minecraft:block_state_property".to_string());
        term.put("block", NbtTag::String(block));
        if let Some(state) = term.child_tags.remove("state") {
            term.put("properties", state);
        }
        return;
    }

    term.child_tags.clear();
    term.put_string("condition", "minecraft:all_of".to_string());
    term.put_list("terms", Vec::new());
}

fn update_enchantment_term(term: &mut NbtCompound) {
    let type_name = term.get_string("type").map(str::to_string);
    if let Some(type_name) = type_name {
        term.put_string("condition", type_name.clone());
        if type_name == "damage_source_properties"
            && let Some(NbtTag::Compound(predicate)) = term.child_tags.get_mut("predicate")
            && let Some(NbtTag::List(tags)) = predicate.child_tags.get_mut("tags")
        {
            update_tag_key(tags);
        }
        if type_name == "match_block" {
            update_match_block(term);
            return;
        }
    }
    if let Some(NbtTag::List(terms)) = term.child_tags.get_mut("terms") {
        for child in terms {
            if let NbtTag::Compound(child) = child {
                update_enchantment_term(child);
            }
        }
    }
}

fn rewrite_registry_entry(registry: &str, tag: &mut NbtTag) {
    let NbtTag::Compound(root) = tag else {
        return;
    };
    if registry == "dimension_type" || registry == "worldgen/biome" {
        handle_environment_attributes(root);
    }
    if registry == "trim_material" {
        if let Some(NbtTag::String(palette_id)) = root.child_tags.remove("palette_id") {
            root.put_string("asset_name", trim_asset_name(&palette_id).to_string());
        }
    }
    if registry == "enchantment" {
        for value in root.child_tags.values_mut() {
            if let NbtTag::Compound(term) = value {
                update_enchantment_term(term);
            }
        }
    }
    if registry == "worldgen/block_state_provider" {
        update_block_state_provider(root);
    }
    if let Some(NbtTag::Compound(data)) = root.child_tags.get_mut("particle")
        && let Some(NbtTag::Compound(state)) = data.child_tags.get_mut("block_state")
    {
        update_full_block_state(state);
    }
}

fn registry_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let payload = wrapper.remaining();
    let mut cursor = payload;
    let registry: String = cursor.get_str()?.into();
    if REMOVED_REGISTRIES.contains(&registry.as_str()) {
        wrapper.cancel();
        return Ok(());
    }

    let count = cursor.get_var_int()?.0;
    if !(0..=65536).contains(&count) {
        return Err(TranslateError::Unsupported("registry entry count"));
    }
    let mut out = Vec::with_capacity(payload.len());
    out.write_string(&registry)?;
    out.write_var_int(&VarInt(count))?;
    for _ in 0..count {
        let name: String = cursor.get_str()?.into();
        let has_data = cursor.get_bool()?;
        out.write_string(&name)?;
        out.write_bool(has_data)?;
        if has_data {
            let mut data = NbtT::for_version(ctx.layout).read(&mut cursor)?;
            if let Some(data) = data.as_mut() {
                rewrite_registry_entry(
                    registry.strip_prefix("minecraft:").unwrap_or(&registry),
                    data,
                );
            }
            NbtT::for_version(ctx.layout).write(&mut out, &data)?;
        }
    }
    if !cursor.is_empty() {
        return Err(TranslateError::TrailingBytes(cursor.len()));
    }
    wrapper.replace_remaining(out);
    Ok(())
}

fn update_tags(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if let Some(out) = rewrite_tags(wrapper.remaining(), ctx.layout) {
        wrapper.replace_remaining(out);
    } else {
        wrapper.passthrough_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_protocol::ser::NetworkWriteExt;

    #[test]
    fn old_block_tag_alias_is_renamed_without_touching_member_ids() {
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(1)).unwrap(); // group count
        payload.write_string("minecraft:block").unwrap();
        payload.write_var_int(&VarInt(1)).unwrap(); // tag count
        payload
            .write_string("minecraft:convertible_to_mud")
            .unwrap();
        payload.write_var_int(&VarInt(2)).unwrap();
        payload.write_var_int(&VarInt(5)).unwrap();
        payload.write_var_int(&VarInt(8)).unwrap();

        let out = rewrite_tags(&payload, V::V_26_2).unwrap();
        let mut cursor = out.as_slice();
        assert_eq!(cursor.get_var_int().unwrap().0, 1);
        assert_eq!(cursor.get_str().unwrap().as_ref(), "minecraft:block");
        assert_eq!(cursor.get_var_int().unwrap().0, 1);
        assert_eq!(
            cursor.get_str().unwrap().as_ref(),
            "minecraft:convertable_to_mud"
        );
        assert_eq!(cursor.get_var_int().unwrap().0, 2);
        assert_eq!(cursor.get_var_int().unwrap().0, 5);
        assert_eq!(cursor.get_var_int().unwrap().0, 8);
        assert!(cursor.is_empty());
    }

    #[test]
    fn environment_append_and_overlay_modifiers_become_override() {
        let mut attribute = NbtCompound::new();
        attribute.put_string("modifier", "append".to_string());
        let mut attributes = NbtCompound::new();
        attributes.put_compound("minecraft:temperature", attribute);
        let mut root = NbtCompound::new();
        root.put_compound("attributes", attributes);
        handle_environment_attributes(&mut root);
        assert_eq!(
            root.get_compound("attributes")
                .unwrap()
                .get_compound("minecraft:temperature")
                .unwrap()
                .get_string("modifier"),
            Some("override")
        );
    }

    #[test]
    fn trim_palette_name_loses_the_trim_prefix_and_namespace() {
        assert_eq!(trim_asset_name("minecraft:trim/diamond"), "diamond");
        assert_eq!(trim_asset_name("modded:trim/ruby"), "ruby");
    }
}
