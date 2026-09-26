use std::sync::OnceLock;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::enchantment::Enchantment;
use pumpkin_data::potion::Potion;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt};
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::item_component::{
    legacy_modifier_uuid, registry_entry_id, registry_entry_name,
};
use crate::api::rewriter::item_shape::{self, Shape};
use crate::api::types::{Item, ItemComponent, TEMPLATE_ITEM, WireType};
use crate::api::{ComposedMappings, MappingData};
use crate::data::entity_types::stand_in_type_for_version;

/// The enchantments every client from 1.13.2 up has, by registry name
/// (`md('1.13.2').enchantmentsArray`).
static ENCHANTMENTS_1_13: &[&str] = &[
    "aqua_affinity",
    "bane_of_arthropods",
    "binding_curse",
    "blast_protection",
    "channeling",
    "depth_strider",
    "efficiency",
    "feather_falling",
    "fire_aspect",
    "fire_protection",
    "flame",
    "fortune",
    "frost_walker",
    "impaling",
    "infinity",
    "knockback",
    "looting",
    "loyalty",
    "luck_of_the_sea",
    "lure",
    "mending",
    "power",
    "projectile_protection",
    "protection",
    "punch",
    "respiration",
    "riptide",
    "sharpness",
    "silk_touch",
    "smite",
    "sweeping",
    "thorns",
    "unbreaking",
    "vanishing_curse",
];

/// The crossbow enchantments, added with the crossbow in 1.14.
static ENCHANTMENTS_1_14: &[&str] = &["multishot", "piercing", "quick_charge"];

/// Soul speed, in `md('1.16.5')` and not in `md('1.16.1')`.
static ENCHANTMENTS_1_16: &[&str] = &["soul_speed"];

/// Swift sneak, in `md('1.19')` and not in `md('1.18.2')`.
static ENCHANTMENTS_1_19: &[&str] = &["swift_sneak"];

/// `minecraft-data` reports `sweeping` up to 1.20.4 and `sweeping_edge` from
/// 1.20.6.
const NATIVE_SWEEPING: &str = "sweeping_edge";
const LEGACY_SWEEPING: &str = "sweeping";

/// The potions added in 1.21; a bottle holding one loses its `Potion` tag
/// rather than taking a substitute.
static POTIONS_1_21: &[&str] = &["infested", "oozing", "weaving", "wind_charged"];

fn enchantment_exists_on(name: &str, version: V) -> bool {
    ENCHANTMENTS_1_13.contains(&name)
        || (version >= V::V_1_14 && ENCHANTMENTS_1_14.contains(&name))
        || (version >= V::V_1_16 && ENCHANTMENTS_1_16.contains(&name))
        || (version >= V::V_1_19 && ENCHANTMENTS_1_19.contains(&name))
}

fn legacy_enchantment_name(enchantment: &Enchantment, version: V) -> Option<&'static str> {
    let key = if enchantment.registry_key == NATIVE_SWEEPING {
        LEGACY_SWEEPING
    } else {
        enchantment.registry_key
    };
    enchantment_exists_on(key, version).then_some(key)
}

fn native_enchantment(id: &str) -> Option<&'static Enchantment> {
    let bare = id.strip_prefix("minecraft:").unwrap_or(id);
    if bare == LEGACY_SWEEPING {
        return Enchantment::from_name(NATIVE_SWEEPING);
    }
    Enchantment::from_name(bare)
}

/// The category prefixed attribute names in the 1.20.4 server jar, which are
/// what `md('1.20.1').attributesArray[].resource` holds, keyed by the 26.3
/// registry id the wire carries.
static LEGACY_ATTRIBUTE_NAMES: &[(u8, &str)] = &[
    (Attributes::ARMOR.id, "generic.armor"),
    (Attributes::ARMOR_TOUGHNESS.id, "generic.armor_toughness"),
    (Attributes::ATTACK_DAMAGE.id, "generic.attack_damage"),
    (Attributes::ATTACK_KNOCKBACK.id, "generic.attack_knockback"),
    (Attributes::ATTACK_SPEED.id, "generic.attack_speed"),
    (Attributes::FLYING_SPEED.id, "generic.flying_speed"),
    (Attributes::FOLLOW_RANGE.id, "generic.follow_range"),
    (
        Attributes::KNOCKBACK_RESISTANCE.id,
        "generic.knockback_resistance",
    ),
    (Attributes::LUCK.id, "generic.luck"),
    (Attributes::MAX_HEALTH.id, "generic.max_health"),
    (Attributes::MOVEMENT_SPEED.id, "generic.movement_speed"),
    (Attributes::JUMP_STRENGTH.id, "horse.jump_strength"),
    (
        Attributes::SPAWN_REINFORCEMENTS.id,
        "zombie.spawn_reinforcements",
    ),
];

/// The one attribute here that is not there for the whole range: it arrives
/// in 1.20.2.
const LEGACY_MAX_ABSORPTION: &str = "generic.max_absorption";

fn legacy_attribute_name(id: i32, version: V) -> Option<&'static str> {
    let id = u8::try_from(id).ok()?;
    if id == Attributes::MAX_ABSORPTION.id {
        return (version >= V::V_1_20_2).then_some(LEGACY_MAX_ABSORPTION);
    }
    LEGACY_ATTRIBUTE_NAMES
        .iter()
        .find(|(native, _)| *native == id)
        .map(|(_, legacy)| *legacy)
}

/// Clients before 1.20.5 only have the six equipment slots; a modifier on a
/// group slot has no spelling there.
enum LegacySlot {
    Everywhere,
    Named(&'static str),
}

/// The slot ids `pumpkin_protocol` writes for `attribute_modifiers`.
const fn legacy_slot(slot: i32) -> Option<LegacySlot> {
    match slot {
        0 => Some(LegacySlot::Everywhere),
        1 => Some(LegacySlot::Named("mainhand")),
        2 => Some(LegacySlot::Named("offhand")),
        4 => Some(LegacySlot::Named("feet")),
        5 => Some(LegacySlot::Named("legs")),
        6 => Some(LegacySlot::Named("chest")),
        7 => Some(LegacySlot::Named("head")),
        _ => None,
    }
}

/// Reads a tag clients write as an int but may narrow to a byte or short.
fn extract_int_like(tag: &NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Byte(v) => Some(i32::from(*v)),
        NbtTag::Short(v) => Some(i32::from(*v)),
        NbtTag::Int(v) => Some(*v),
        NbtTag::Long(v) => i32::try_from(*v).ok(),
        _ => None,
    }
}

/// Text that is not valid component JSON is kept as plain text, which is what
/// the client itself shows for it.
fn text_from_json(raw: &str) -> TextComponent {
    serde_json::from_str(raw).unwrap_or_else(|_| TextComponent::text(raw.to_owned()))
}

fn text_to_json(text: &TextComponent, version: V, ids: &ComposedMappings) -> String {
    sanitize_text(text, version, ids).to_json_for_version(&version)
}

/// `display.Lore` holds JSON text components from 1.14; on 1.13 the entries
/// are plain strings.
fn lore_line(text: &TextComponent, version: V, ids: &ComposedMappings) -> String {
    if version >= V::V_1_14 {
        text_to_json(text, version, ids)
    } else {
        text.clone().get_text()
    }
}

fn lore_from_wire(raw: &str, version: V) -> TextComponent {
    if version >= V::V_1_14 {
        text_from_json(raw)
    } else {
        TextComponent::text(raw.to_owned())
    }
}

/// Drops hover events naming an item or entity the target has no id for; the
/// client resolves those names while decoding and one unknown name fails the
/// packet.
fn sanitize_text(text: &TextComponent, version: V, ids: &ComposedMappings) -> TextComponent {
    let mut out = text.clone();
    sanitize_base(&mut out.0, version, ids);
    out
}

fn hover_exists_on(
    hover: &pumpkin_util::text::hover::HoverEvent,
    version: V,
    ids: &ComposedMappings,
) -> bool {
    use pumpkin_util::text::hover::HoverEvent;
    match hover {
        HoverEvent::ShowText { .. } => true,
        HoverEvent::ShowItem { id, .. } => pumpkin_data::item::Item::from_registry_key(id)
            .is_some_and(|item| ids.items.map(u32::from(item.id)).is_some()),
        HoverEvent::ShowEntity { id, .. } => pumpkin_data::entity::EntityType::from_name(id)
            .map(|entity| stand_in_type_for_version(entity.id, version))
            .is_some_and(|entity| ids.entities.map(u32::from(entity)).is_some()),
    }
}

fn sanitize_base(
    base: &mut pumpkin_util::text::TextComponentBase,
    version: V,
    ids: &ComposedMappings,
) {
    use pumpkin_util::text::hover::HoverEvent;
    if let Some(HoverEvent::ShowEntity { id, .. }) = base.style.hover_event.as_mut()
        && let Some(entity) = pumpkin_data::entity::EntityType::from_name(id)
    {
        let stand_in = stand_in_type_for_version(entity.id, version);
        if stand_in != entity.id
            && let Some(mapped) = pumpkin_data::entity::EntityType::from_raw(stand_in)
        {
            let resource_name = mapped.resource_name;
            *id = if resource_name.contains(':') {
                resource_name.to_owned().into()
            } else {
                format!("minecraft:{resource_name}").into()
            };
        }
    }
    if base
        .style
        .hover_event
        .as_ref()
        .is_some_and(|hover| !hover_exists_on(hover, version, ids))
    {
        base.style.hover_event = None;
    }
    match base.style.hover_event.as_mut() {
        Some(HoverEvent::ShowText { value }) => {
            for child in value {
                sanitize_base(child, version, ids);
            }
        }
        Some(HoverEvent::ShowEntity {
            name: Some(name), ..
        }) => {
            for child in name {
                sanitize_base(child, version, ids);
            }
        }
        _ => {}
    }
    if let pumpkin_util::text::TextContent::Translate { with, .. } = base.content.as_mut() {
        for arg in with {
            sanitize_base(arg, version, ids);
        }
    }
    for child in &mut base.extra {
        sanitize_base(child, version, ids);
    }
}

fn legacy_enchantment_list(
    pairs: &[(i32, i32)],
    version: V,
    unsupported_lore: &mut Vec<NbtTag>,
) -> Vec<NbtTag> {
    let mut tags = Vec::with_capacity(pairs.len());
    for (id, level) in pairs {
        let Some(enchantment) = u8::try_from(*id).ok().and_then(Enchantment::from_id) else {
            continue;
        };
        let Some(name) = legacy_enchantment_name(enchantment, version) else {
            if version < V::V_1_20_5
                && let Some(static_id) = u32::try_from(*id).ok().and_then(|id| {
                    MappingData::get()
                        .composed(V::V_1_20_5)
                        .enchantments
                        .map(id)
                })
                && let Some(mapped_name) = MappingData::get()
                    .step(V::V_1_20_5)
                    .enchantment_names
                    .get(&static_id)
            {
                unsupported_lore.push(NbtTag::String(
                    enchantment_lore_line(mapped_name, *level, version).into(),
                ));
            }
            continue;
        };
        let mut entry = NbtCompound::new();
        entry.put_string("id", format!("minecraft:{name}"));
        entry.put_short("lvl", (*level).clamp(0, i32::from(i16::MAX)) as i16);
        tags.push(NbtTag::Compound(entry));
    }
    tags
}

pub(crate) fn unsupported_enchantment_lore(
    bytes: &[u8],
    target: V,
    ids: &ComposedMappings,
) -> Vec<String> {
    if target < V::V_1_20_5 {
        return Vec::new();
    }
    var_int_pairs(bytes)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, level)| {
            let source_id = u32::try_from(id).ok()?;
            if ids.enchantments.map(source_id).is_some() {
                return None;
            }
            let enchantment = u8::try_from(id).ok().and_then(Enchantment::from_id)?;
            let key = enchantment
                .registry_key
                .strip_prefix("minecraft:")
                .unwrap_or(enchantment.registry_key);
            let name = key
                .split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().collect::<String>() + chars.as_str()
                    })
                })
                .collect::<Vec<_>>()
                .join(" ");
            Some(enchantment_lore_line(&name, level, target))
        })
        .collect()
}

pub(crate) fn append_enchantment_lore(
    existing: Option<&[u8]>,
    lines: &[String],
) -> Option<Vec<u8>> {
    if lines.is_empty() {
        return existing.map(ToOwned::to_owned);
    }
    let mut existing_lines = Vec::new();
    if let Some(existing) = existing {
        let mut cursor = existing;
        let count = cursor.get_var_int().ok()?.0;
        if !(0..=4096).contains(&count) {
            return None;
        }
        for _ in 0..count {
            existing_lines.push(cursor.get_nbt(&V::V_26_2).ok()??);
        }
        if !cursor.is_empty() {
            return None;
        }
    }

    let count = lines.len().checked_add(existing_lines.len())?;
    let mut output = Vec::new();
    output
        .write_var_int(&VarInt(i32::try_from(count).ok()?))
        .ok()?;
    for line in lines {
        output
            .write_nbt_with_version(Some(&NbtTag::String(line.clone().into())), &V::V_26_2)
            .ok()?;
    }
    for line in existing_lines {
        output
            .write_nbt_with_version(Some(&line), &V::V_26_2)
            .ok()?;
    }
    Some(output)
}

fn enchantment_lore_line(name: &str, level: i32, version: V) -> String {
    let level = match level {
        1 => "I".to_owned(),
        2 => "II".to_owned(),
        3 => "III".to_owned(),
        4 => "IV".to_owned(),
        5 => "V".to_owned(),
        6 => "VI".to_owned(),
        7 => "VII".to_owned(),
        8 => "VIII".to_owned(),
        9 => "IX".to_owned(),
        10 => "X".to_owned(),
        other => other.to_string(),
    };
    let text = format!("{name} {level}");
    if version >= V::V_1_14 {
        serde_json::json!({"text": text, "color": "gray"}).to_string()
    } else {
        text
    }
}

fn native_enchantment_list(tags: &[NbtTag]) -> Vec<(i32, i32)> {
    let mut list = Vec::with_capacity(tags.len());
    for tag in tags {
        let Some(entry) = tag.extract_compound() else {
            continue;
        };
        let Some(enchantment) = entry.get_string("id").and_then(native_enchantment) else {
            continue;
        };
        let level = entry.get("lvl").and_then(extract_int_like).unwrap_or(1);
        list.push((i32::from(enchantment.id), level));
    }
    list
}

fn legacy_potion_name(potion_id: i32, version: V) -> Option<String> {
    let id = u8::try_from(potion_id).ok()?;
    let potion = Potion::from_id(id)?;
    let known_here = version >= V::V_1_21 || !POTIONS_1_21.contains(&potion.name);
    known_here.then(|| format!("minecraft:{}", potion.name))
}

/// One entry of a 26.3 `attribute_modifiers` payload.
struct Modifier {
    attribute: i32,
    id: String,
    amount: f64,
    operation: i32,
    slot: i32,
}

fn legacy_attribute_entry(modifier: &Modifier, index: i32, version: V) -> Option<NbtCompound> {
    let name = legacy_attribute_name(modifier.attribute, version)?;
    let slot = legacy_slot(modifier.slot)?;
    if !(0..=2).contains(&modifier.operation) {
        return None;
    }
    let mut entry = NbtCompound::new();
    entry.put_string("AttributeName", name.to_owned());
    entry.put_string("Name", modifier.id.clone());
    entry.put_double("Amount", modifier.amount);
    entry.put_int("Operation", modifier.operation);
    let (high, low) = legacy_modifier_uuid(&modifier.id, index).as_u64_pair();
    if version >= V::V_1_16 {
        // 1.16 replaced the two longs with an int array, vanilla's `ItemStackUUIDFix`.
        entry.put(
            "UUID",
            NbtTag::IntArray(vec![
                (high >> 32) as i32,
                (high & 0xffff_ffff) as i32,
                (low >> 32) as i32,
                (low & 0xffff_ffff) as i32,
            ]),
        );
    } else {
        entry.put_long("UUIDMost", high as i64);
        entry.put_long("UUIDLeast", low as i64);
    }
    if let LegacySlot::Named(slot) = slot {
        entry.put_string("Slot", slot.to_owned());
    }
    Some(entry)
}

/// A game profile as the 26.3 payload carries it.
#[derive(Default)]
struct Profile {
    name: Option<String>,
    id: Option<[i32; 4]>,
    properties: Vec<(String, String, Option<String>)>,
}

fn uuid_words(uuid: uuid::Uuid) -> [i32; 4] {
    let bits = uuid.as_u128();
    [
        (bits >> 96) as i32,
        (bits >> 64) as i32,
        (bits >> 32) as i32,
        bits as i32,
    ]
}

fn read_profile(r: &mut &[u8]) -> Option<Profile> {
    let mut profile = Profile::default();
    if r.get_var_int().ok()?.0 == 0 {
        profile.id = Some(uuid_words(r.get_uuid().ok()?));
        profile.name = Some(r.get_str().ok()?.into());
    } else {
        if r.get_bool().ok()? {
            profile.name = Some(r.get_str().ok()?.into());
        }
        if r.get_bool().ok()? {
            profile.id = Some(uuid_words(r.get_uuid().ok()?));
        }
    }
    let count = r.get_var_int().ok()?.0;
    for _ in 0..count {
        let name = r.get_str().ok()?.into();
        let value = r.get_str().ok()?.into();
        let signature = if r.get_bool().ok()? {
            Some(r.get_str().ok()?.into())
        } else {
            None
        };
        profile.properties.push((name, value, signature));
    }
    Some(profile)
}

fn write_profile(profile: &Profile, out: &mut Vec<u8>) -> Option<()> {
    out.write_var_int(&VarInt(1)).ok()?;
    match &profile.name {
        Some(name) => {
            out.write_bool(true).ok()?;
            out.write_string(name).ok()?;
        }
        None => out.write_bool(false).ok()?,
    }
    match &profile.id {
        Some(id) => {
            out.write_bool(true).ok()?;
            let bits = (u128::from(id[0] as u32) << 96)
                | (u128::from(id[1] as u32) << 64)
                | (u128::from(id[2] as u32) << 32)
                | u128::from(id[3] as u32);
            out.write_uuid(&uuid::Uuid::from_u128(bits)).ok()?;
        }
        None => out.write_bool(false).ok()?,
    }
    out.write_var_int(&VarInt(
        i32::try_from(profile.properties.len()).unwrap_or(0),
    ))
    .ok()?;
    for (name, value, signature) in &profile.properties {
        out.write_string(name).ok()?;
        out.write_string(value).ok()?;
        match signature {
            Some(signature) => {
                out.write_bool(true).ok()?;
                out.write_string(signature).ok()?;
            }
            None => out.write_bool(false).ok()?,
        }
    }
    // The skin patch: texture, cape, elytra and model, none of them set.
    for _ in 0..4 {
        out.write_bool(false).ok()?;
    }
    Some(())
}

fn legacy_skull_owner(profile: &Profile, version: V) -> Option<NbtCompound> {
    let mut owner = NbtCompound::new();
    if let Some(name) = &profile.name {
        owner.put_string("Name", name.clone());
    }
    if let Some(id) = &profile.id {
        if version >= V::V_1_16 {
            owner.put("Id", NbtTag::IntArray(id.to_vec()));
        } else {
            let high = (u64::from(id[0] as u32) << 32) | u64::from(id[1] as u32);
            let low = (u64::from(id[2] as u32) << 32) | u64::from(id[3] as u32);
            owner.put_string("Id", uuid::Uuid::from_u64_pair(high, low).to_string());
        }
    }
    let textures: Vec<NbtTag> = profile
        .properties
        .iter()
        .filter(|(name, _, _)| name == "textures")
        .map(|(_, value, signature)| {
            let mut texture = NbtCompound::new();
            texture.put_string("Value", value.clone());
            if let Some(signature) = signature {
                texture.put_string("Signature", signature.clone());
            }
            NbtTag::Compound(texture)
        })
        .collect();
    if !textures.is_empty() {
        let mut properties = NbtCompound::new();
        properties.put_list("textures", textures);
        owner.put_compound("Properties", properties);
    }
    (!owner.is_empty()).then_some(owner)
}

fn find(added: &[ItemComponent], component: DataComponent) -> Option<&[u8]> {
    let id = i32::from(component.to_id());
    added
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.data.as_slice())
}

fn nbt_of(bytes: &[u8]) -> Option<NbtTag> {
    let mut cursor = bytes;
    cursor.get_nbt(&V::V_26_2).ok().flatten()
}

fn text_of(bytes: &[u8]) -> TextComponent {
    nbt_of(bytes).map_or_else(TextComponent::empty, |tag| TextComponent::from_nbt(&tag))
}

fn var_int_pairs(bytes: &[u8]) -> Option<Vec<(i32, i32)>> {
    let mut r = bytes;
    let count = r.get_var_int().ok()?.0;
    let mut pairs = Vec::new();
    for _ in 0..count {
        pairs.push((r.get_var_int().ok()?.0, r.get_var_int().ok()?.0));
    }
    Some(pairs)
}

/// The single int a legacy `CustomModelData` tag holds, if the 26.3 component
/// holds exactly that and nothing else.
pub(crate) fn legacy_custom_model_data(bytes: &[u8]) -> Option<i32> {
    let mut r = bytes;
    if r.get_var_int().ok()?.0 != 1 {
        return None;
    }
    let value = f32::from_bits(r.get_i32_be().ok()? as u32);
    for _ in 0..3 {
        if r.get_var_int().ok()?.0 != 0 {
            return None;
        }
    }
    if !r.is_empty() {
        return None;
    }
    let rounded = value as i32;
    (value == rounded as f32).then_some(rounded)
}

fn read_written_book(r: &mut &[u8]) -> Option<(String, String, Vec<String>)> {
    let title = r.get_str().ok()?.into();
    if r.get_bool().ok()? {
        r.get_str().ok()?;
    }
    let author = r.get_str().ok()?.into();
    r.get_var_int().ok()?;
    let count = r.get_var_int().ok()?.0;
    let mut pages = Vec::new();
    for _ in 0..count {
        let tag = r.get_nbt(&V::V_26_2).ok()?;
        let text = tag.map_or_else(TextComponent::empty, |tag| TextComponent::from_nbt(&tag));
        if r.get_bool().ok()? {
            r.get_nbt(&V::V_26_2).ok()?;
        }
        pages.push(text.get_text());
    }
    Some((title, author, pages))
}

fn read_writable_book(r: &mut &[u8]) -> Option<Vec<String>> {
    let count = r.get_var_int().ok()?.0;
    let mut pages = Vec::new();
    for _ in 0..count {
        pages.push(r.get_str().ok()?.into());
        if r.get_bool().ok()? {
            r.get_str().ok()?;
        }
    }
    Some(pages)
}

fn read_modifiers(r: &mut &[u8]) -> Option<Vec<Modifier>> {
    let count = r.get_var_int().ok()?.0;
    let mut modifiers = Vec::new();
    for _ in 0..count {
        let attribute = r.get_var_int().ok()?.0;
        let id = r.get_str().ok()?.into();
        let amount = r.get_f64_be().ok()?;
        let operation = r.get_var_int().ok()?.0;
        let slot = r.get_var_int().ok()?.0;
        if r.get_var_int().ok()?.0 == 2 {
            r.get_nbt(&V::V_26_2).ok()?;
        }
        modifiers.push(Modifier {
            attribute,
            id,
            amount,
            operation,
            slot,
        });
    }
    Some(modifiers)
}

fn component_hidden_in_tooltip(added: &[ItemComponent], component: DataComponent) -> bool {
    let Some(tooltip) = find(added, DataComponent::TooltipDisplay) else {
        return false;
    };
    let mut cursor = tooltip;
    let Ok(hide_all) = cursor.get_bool() else {
        return true;
    };
    if hide_all {
        return true;
    }
    let Ok(count) = cursor.get_var_int() else {
        return true;
    };
    if !(0..=4096).contains(&count.0) {
        return true;
    }
    (0..count.0).any(|_| {
        cursor
            .get_var_int()
            .is_ok_and(|id| id.0 == i32::from(component.to_id()))
    })
}

fn legacy_custom_potion_effects(r: &mut &[u8], version: V) -> Option<Vec<NbtTag>> {
    let count = r.get_var_int().ok()?.0;
    if !(0..=4096).contains(&count) {
        return None;
    }
    let mut effects = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let source_id = r.get_var_int().ok()?.0;
        let mut effect_data = Vec::new();
        let data_start = *r;
        item_shape::skip_effect_parameters(r).ok()?;
        effect_data.extend_from_slice(&data_start[..data_start.len() - r.len()]);
        let name = registry_entry_name(V::V_26_3, "mob_effect", source_id);
        let target_id = name.and_then(|name| registry_entry_id(version, "mob_effect", name));
        if let Some(target_id) = target_id
            && let Some(data) = potion_effect_data_to_nbt(&effect_data)
        {
            let mut effect = data;
            effect.put_byte("Id", target_id as i8);
            effects.push(NbtTag::Compound(effect));
        }
    }
    if r.get_bool().ok()? {
        r.get_str().ok()?; // Custom potion name.
    }
    r.is_empty().then_some(effects)
}

fn potion_effect_data_to_nbt(bytes: &[u8]) -> Option<NbtCompound> {
    let mut cursor = bytes;
    let amplifier = cursor.get_var_int().ok()?.0;
    let duration = cursor.get_var_int().ok()?.0;
    let ambient = cursor.get_bool().ok()?;
    let show_particles = cursor.get_bool().ok()?;
    let show_icon = cursor.get_bool().ok()?;
    let has_hidden = cursor.get_bool().ok()?;
    let mut effect = NbtCompound::new();
    effect.put_byte("Amplifier", amplifier as i8);
    effect.put_int("Duration", duration);
    effect.put_bool("Ambient", ambient);
    effect.put_bool("ShowParticles", show_particles);
    effect.put_bool("ShowIcon", show_icon);
    if has_hidden {
        let hidden_start = cursor;
        item_shape::skip_effect_parameters(&mut cursor).ok()?;
        let hidden_data = &hidden_start[..hidden_start.len() - cursor.len()];
        effect.put_compound("HiddenEffect", potion_effect_data_to_nbt(hidden_data)?);
    }
    cursor.is_empty().then_some(effect)
}

static ITEMS_1_16_2: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_17: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_18: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_19: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_19_3: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_19_4: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_20: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_20_2: OnceLock<Vec<String>> = OnceLock::new();
static ITEMS_1_20_3: OnceLock<Vec<String>> = OnceLock::new();

fn legacy_item_names(version: V) -> Option<&'static [String]> {
    let (cache, data): (&OnceLock<Vec<String>>, &str) = if version < V::V_1_17 {
        (
            &ITEMS_1_16_2,
            include_str!("../../../assets/items/1_16_2_items.json"),
        )
    } else if version < V::V_1_18 {
        (
            &ITEMS_1_17,
            include_str!("../../../assets/items/1_17_items.json"),
        )
    } else if version < V::V_1_19 {
        (
            &ITEMS_1_18,
            include_str!("../../../assets/items/1_18_items.json"),
        )
    } else if version < V::V_1_19_3 {
        (
            &ITEMS_1_19,
            include_str!("../../../assets/items/1_19_items.json"),
        )
    } else if version < V::V_1_19_4 {
        (
            &ITEMS_1_19_3,
            include_str!("../../../assets/items/1_19_3_items.json"),
        )
    } else if version < V::V_1_20 {
        (
            &ITEMS_1_19_4,
            include_str!("../../../assets/items/1_19_4_items.json"),
        )
    } else if version < V::V_1_20_2 {
        (
            &ITEMS_1_20,
            include_str!("../../../assets/items/1_20_items.json"),
        )
    } else if version < V::V_1_20_3 {
        (
            &ITEMS_1_20_2,
            include_str!("../../../assets/items/1_20_2_items.json"),
        )
    } else if version < V::V_1_20_5 {
        (
            &ITEMS_1_20_3,
            include_str!("../../../assets/items/1_20_3_items.json"),
        )
    } else {
        return None;
    };
    Some(
        cache
            .get_or_init(|| serde_json::from_str(data).expect("bundled item registry is valid"))
            .as_slice(),
    )
}

fn legacy_item_stack_tag(
    item: &Item,
    version: V,
    ids: &ComposedMappings,
    slot: Option<i8>,
) -> Option<NbtTag> {
    let Item::Structured {
        count, id, added, ..
    } = item
    else {
        return None;
    };
    let target_id = usize::try_from(map_item_id(&ids.items, *id)?).ok()?;
    let name = legacy_item_names(version)?.get(target_id)?;
    let mut tag = NbtCompound::new();
    tag.put_string(
        "id",
        if name.contains(':') {
            name.clone()
        } else {
            format!("minecraft:{name}")
        },
    );
    tag.put_byte("Count", i8::try_from(*count).unwrap_or(i8::MAX));
    if let Some(slot) = slot {
        tag.put_byte("Slot", slot);
    }
    if let Some(components) = components_to_nbt(added, version, ids) {
        if !components.is_empty() {
            tag.put_compound("tag", components);
        }
    }
    Some(NbtTag::Compound(tag))
}

fn map_item_id(mapping: &crate::api::IdMapping, id: i32) -> Option<i32> {
    i32::try_from(mapping.map(u32::try_from(id).ok()?)?).ok()
}

fn legacy_template_item_list(
    bytes: &[u8],
    version: V,
    ids: &ComposedMappings,
    optional: bool,
) -> Option<Vec<NbtTag>> {
    let mut cursor = bytes;
    let count = cursor.get_var_int().ok()?.0;
    if !(0..=4096).contains(&count) {
        return None;
    }
    let mut items = Vec::new();
    for index in 0..count {
        if optional && !cursor.get_bool().ok()? {
            continue;
        }
        let item = TEMPLATE_ITEM.read(&mut cursor).ok()?;
        let slot = if optional {
            Some(i8::try_from(index).ok()?)
        } else {
            None
        };
        if let Some(item) = legacy_item_stack_tag(&item, version, ids, slot) {
            items.push(item);
        }
    }
    cursor.is_empty().then_some(items)
}

/// The NBT a client below 1.20.5 should receive for a stack's components, or
/// `None` when none of them can be expressed there.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn components_to_nbt(
    added: &[ItemComponent],
    version: V,
    ids: &ComposedMappings,
) -> Option<NbtCompound> {
    // `custom_data` holds the tags with no component of their own, so it is
    // the base every real component writes over.
    let mut root = find(added, DataComponent::CustomData)
        .and_then(nbt_of)
        .and_then(|tag| match tag {
            NbtTag::Compound(compound) => Some(compound),
            _ => None,
        })
        .unwrap_or_default();
    let mut hide_flags = root
        .get("HideFlags")
        .and_then(extract_int_like)
        .unwrap_or(0);
    // Unknown display children belong to custom_data too. Keep them as a base
    // and let the actual item components replace only their legacy keys.
    let mut display = root.get_compound("display").cloned().unwrap_or_default();
    let mut unsupported_enchantment_lore = Vec::new();

    for entry in added {
        let Some(component) = u8::try_from(entry.id)
            .ok()
            .and_then(DataComponent::try_from_id)
        else {
            continue;
        };
        let mut r = entry.data.as_slice();
        match component {
            DataComponent::Damage => {
                if let Ok(damage) = r.get_var_int() {
                    root.put_int("Damage", damage.0);
                }
            }
            DataComponent::RepairCost => {
                if let Ok(cost) = r.get_var_int() {
                    root.put_int("RepairCost", cost.0);
                }
            }
            DataComponent::Unbreakable => root.put_bool("Unbreakable", true),
            DataComponent::CustomName => {
                display.put_string("Name", text_to_json(&text_of(&entry.data), version, ids));
            }
            DataComponent::Lore => {
                let Ok(count) = r.get_var_int() else { continue };
                let mut lines = Vec::new();
                for _ in 0..count.0 {
                    let Ok(tag) = r.get_nbt(&V::V_26_2) else {
                        break;
                    };
                    let text =
                        tag.map_or_else(TextComponent::empty, |tag| TextComponent::from_nbt(&tag));
                    lines.push(NbtTag::String(lore_line(&text, version, ids).into()));
                }
                if !lines.is_empty() {
                    display.put_list("Lore", lines);
                }
            }
            DataComponent::DyedColor => {
                if let Ok(rgb) = r.get_i32_be() {
                    display.put_int("color", rgb);
                }
            }
            DataComponent::Enchantments | DataComponent::StoredEnchantments => {
                let Some(pairs) = var_int_pairs(&entry.data) else {
                    continue;
                };
                let list =
                    legacy_enchantment_list(&pairs, version, &mut unsupported_enchantment_lore);
                if !list.is_empty() {
                    if component == DataComponent::Enchantments {
                        root.put_list("Enchantments", list);
                    } else {
                        root.put_list("StoredEnchantments", list);
                    }
                }
            }
            DataComponent::CustomModelData => {
                if let Some(value) = legacy_custom_model_data(&entry.data) {
                    root.put_int("CustomModelData", value);
                }
            }
            DataComponent::MapId => {
                if let Ok(id) = r.get_var_int() {
                    root.put_int("map", id.0);
                }
            }
            DataComponent::ChargedProjectiles | DataComponent::BundleContents => {
                if let Some(items) = legacy_template_item_list(&entry.data, version, ids, false) {
                    if component == DataComponent::ChargedProjectiles {
                        root.put_list("ChargedProjectiles", items.clone());
                        root.put_bool("Charged", !items.is_empty());
                    } else {
                        root.put_list("Items", items);
                    }
                }
            }
            DataComponent::Container => {
                if let Some(items) = legacy_template_item_list(&entry.data, version, ids, true) {
                    let mut block_entity = root
                        .get_compound("BlockEntityTag")
                        .cloned()
                        .unwrap_or_default();
                    block_entity.put_list("Items", items);
                    root.put_compound("BlockEntityTag", block_entity);
                }
            }
            DataComponent::CanPlaceOn | DataComponent::CanBreak => {
                if let Some(predicates) = legacy_block_predicates(&entry.data, version, ids) {
                    let hidden = component_hidden_in_tooltip(added, component);
                    if component == DataComponent::CanPlaceOn {
                        root.put_list("CanPlaceOn", predicates);
                        if hidden {
                            hide_flags |= 16;
                        }
                    } else {
                        root.put_list("CanDestroy", predicates);
                        if hidden {
                            hide_flags |= 8;
                        }
                    }
                }
            }
            DataComponent::PotionContents => {
                let Ok(has_potion) = r.get_bool() else {
                    continue;
                };
                if has_potion
                    && let Ok(potion) = r.get_var_int()
                    && let Some(name) = legacy_potion_name(potion.0, version)
                {
                    root.put_string("Potion", name);
                }
                if r.get_bool().unwrap_or(false)
                    && let Ok(color) = r.get_i32_be()
                {
                    root.put_int("CustomPotionColor", color);
                }
                if let Some(effects) = legacy_custom_potion_effects(&mut r, version)
                    && !effects.is_empty()
                {
                    root.put_list("CustomPotionEffects", effects);
                }
            }
            DataComponent::Profile => {
                if let Some(owner) =
                    read_profile(&mut r).and_then(|profile| legacy_skull_owner(&profile, version))
                {
                    root.put_compound("SkullOwner", owner);
                }
            }
            DataComponent::BlockEntityData => {
                if r.get_var_int().is_ok()
                    && let Ok(Some(NbtTag::Compound(nbt))) = r.get_nbt(&V::V_26_2)
                {
                    root.put_compound("BlockEntityTag", nbt);
                }
            }
            DataComponent::Trim => {
                if version >= V::V_1_20
                    && let (Ok(material), Ok(pattern)) = (r.get_var_int(), r.get_var_int())
                    && let (Some(material), Some(pattern)) =
                        (material.0.checked_sub(1), pattern.0.checked_sub(1))
                    && r.is_empty()
                    && let (Some(material), Some(pattern)) = (
                        registry_entry_name(V::V_26_3, "trim_material", material),
                        registry_entry_name(V::V_26_3, "trim_pattern", pattern),
                    )
                {
                    let mut trim = NbtCompound::new();
                    trim.put_string("material", format!("minecraft:{material}"));
                    trim.put_string("pattern", format!("minecraft:{pattern}"));
                    root.put_compound("Trim", trim);
                }
            }
            DataComponent::Instrument => {
                if let Ok(holder) = r.get_var_int()
                    && let Some(registry_id) = holder.0.checked_sub(1)
                    && let Some(instrument) =
                        registry_entry_name(V::V_26_3, "instrument", registry_id)
                    && r.is_empty()
                {
                    root.put_string("instrument", format!("minecraft:{instrument}"));
                }
            }
            DataComponent::WrittenBookContent => {
                if let Some((title, author, pages)) = read_written_book(&mut r) {
                    root.put_string("title", title);
                    root.put_string("author", author);
                    root.put_list(
                        "pages",
                        pages
                            .into_iter()
                            .map(|page| NbtTag::String(page.into()))
                            .collect(),
                    );
                    root.put_bool("resolved", true);
                }
            }
            DataComponent::WritableBookContent => {
                if let Some(pages) = read_writable_book(&mut r)
                    && !pages.is_empty()
                {
                    root.put_list(
                        "pages",
                        pages
                            .into_iter()
                            .map(|page| NbtTag::String(page.into()))
                            .collect(),
                    );
                }
            }
            DataComponent::AttributeModifiers => {
                let entries: Vec<NbtTag> = read_modifiers(&mut r)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .filter_map(|(index, modifier)| {
                        legacy_attribute_entry(modifier, i32::try_from(index).unwrap_or(0), version)
                            .map(NbtTag::Compound)
                    })
                    .collect();
                if !entries.is_empty() {
                    root.put_list("AttributeModifiers", entries);
                }
            }
            _ => {}
        }
    }

    if !unsupported_enchantment_lore.is_empty() {
        let mut lore = unsupported_enchantment_lore;
        lore.extend(
            display
                .get_list("Lore")
                .map(|lines| lines.to_vec())
                .unwrap_or_default(),
        );
        display.put_list("Lore", lore);
    }
    if !display.is_empty() {
        root.put_compound("display", display);
    }
    if hide_flags != 0 {
        root.put_byte("HideFlags", hide_flags as i8);
    }
    if root.is_empty() { None } else { Some(root) }
}

/// The root tags this module turns into components; everything else a client
/// sends is kept in `minecraft:custom_data`.
static CONSUMED_ROOT_TAGS: &[&str] = &[
    "BlockEntityTag",
    "CanDestroy",
    "CanPlaceOn",
    "CustomModelData",
    "CustomPotionColor",
    "CustomPotionEffects",
    "custom_potion_effects",
    "Damage",
    "Enchantments",
    "Potion",
    "RepairCost",
    "SkullOwner",
    "StoredEnchantments",
    "Trim",
    "Unbreakable",
    "HideFlags",
    "author",
    "map",
    "pages",
    "resolved",
    "title",
];

fn legacy_block_predicates(
    bytes: &[u8],
    _version: V,
    ids: &ComposedMappings,
) -> Option<Vec<NbtTag>> {
    let mut cursor = bytes;
    let target_block_ids_inverse = ids.blocks.inverse();
    let count = cursor.get_var_int().ok()?.0;
    if !(0..=4096).contains(&count) {
        return None;
    }
    let mut output = Vec::new();
    for _ in 0..count {
        let mut blocks = Vec::new();
        if cursor.get_bool().ok()? {
            let selector = cursor.get_var_int().ok()?.0;
            if selector == 0 {
                blocks.push(format!("#{}", cursor.get_str().ok()?));
            } else if selector > 0 && selector <= 4097 {
                for _ in 0..selector - 1 {
                    let source_id = cursor.get_var_int().ok()?.0;
                    let source_id = u32::try_from(source_id).ok()?;
                    let Some(target_id) = ids.blocks.map(source_id) else {
                        continue;
                    };
                    if target_block_ids_inverse.map(target_id) == Some(source_id)
                        && let Some(name) =
                            registry_entry_name(V::V_26_3, "block", i32::try_from(source_id).ok()?)
                    {
                        blocks.push(format!("minecraft:{name}"));
                    }
                }
            } else {
                return None;
            }
        }

        let mut properties = Vec::new();
        let mut has_unrepresentable_property_range = false;
        if cursor.get_bool().ok()? {
            let property_count = cursor.get_var_int().ok()?.0;
            if !(0..=4096).contains(&property_count) {
                return None;
            }
            for _ in 0..property_count {
                let name = cursor.get_str().ok()?;
                let exact = cursor.get_bool().ok()?;
                if exact {
                    properties.push(format!("{name}={}", cursor.get_str().ok()?));
                } else {
                    if cursor.get_bool().ok()? {
                        cursor.get_str().ok()?; // Lower bound.
                    }
                    if cursor.get_bool().ok()? {
                        cursor.get_str().ok()?; // Upper bound.
                    }
                    has_unrepresentable_property_range = true;
                }
            }
        }

        let has_nbt = cursor.get_bool().ok()?;
        if has_nbt {
            cursor.get_nbt(&V::V_26_3).ok()?;
        }
        let mut component_matchers_cursor = cursor;
        let component_matcher_count = component_matchers_cursor.get_var_int().ok()?.0;
        if !(0..=4096).contains(&component_matcher_count) {
            return None;
        }
        item_shape::skip(&Shape::Array(&Shape::Component), &mut cursor).ok()?;
        let mut item_matchers_cursor = cursor;
        let item_matcher_count = item_matchers_cursor.get_var_int().ok()?.0;
        if !(0..=4096).contains(&item_matcher_count) {
            return None;
        }
        item_shape::skip(&Shape::Array(&Shape::VarInt), &mut cursor).ok()?;
        // Omitting an unsupported predicate must not widen its allow-list.
        if has_nbt
            || has_unrepresentable_property_range
            || component_matcher_count > 0
            || item_matcher_count > 0
            || blocks.is_empty()
        {
            continue;
        }
        for block in blocks {
            let mut value = block;
            if !properties.is_empty() {
                value.push('[');
                value.push_str(&properties.join(","));
                value.push(']');
            }
            output.push(NbtTag::String(value.into()));
        }
    }
    cursor.is_empty().then_some(output)
}

fn legacy_block_predicates_from_nbt(list: &[NbtTag]) -> Option<Vec<u8>> {
    let mut predicates = Vec::new();
    for value in list {
        let Some(raw) = value.extract_string() else {
            continue;
        };
        let (block, state) = raw.split_once('[').map_or((raw, None), |(block, state)| {
            (block, state.strip_suffix(']'))
        });
        let is_tag = block.starts_with('#');
        let identifier = block.strip_prefix('#').unwrap_or(block);
        let bare = identifier.strip_prefix("minecraft:").unwrap_or(identifier);
        let mut predicate = Vec::new();
        predicate.write_bool(true).ok()?; // Holder set is present.
        if is_tag {
            predicate.write_var_int(&VarInt(0)).ok()?;
            predicate.write_string(identifier).ok()?;
        } else {
            let id = registry_entry_id(V::V_26_3, "block", bare)?;
            predicate.write_var_int(&VarInt(2)).ok()?; // One explicit block id.
            predicate.write_var_int(&VarInt(id)).ok()?;
        }

        let properties = state
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|property| property.split_once('='))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        predicate.write_bool(!properties.is_empty()).ok()?;
        if !properties.is_empty() {
            predicate
                .write_var_int(&VarInt(i32::try_from(properties.len()).ok()?))
                .ok()?;
            for (name, value) in properties {
                predicate.write_string(name).ok()?;
                predicate.write_bool(true).ok()?; // Exact property value.
                predicate.write_string(value).ok()?;
            }
        }
        predicate.write_bool(false).ok()?; // No block entity NBT condition.
        predicate.write_var_int(&VarInt(0)).ok()?; // No component matchers.
        predicate.write_var_int(&VarInt(0)).ok()?; // No extra requirements.
        predicates.push(predicate);
    }

    let mut out = Vec::new();
    out.write_var_int(&VarInt(i32::try_from(predicates.len()).ok()?))
        .ok()?;
    for predicate in predicates {
        out.extend(predicate);
    }
    Some(out)
}

fn legacy_potion_effect_from_nbt(tag: &NbtTag, version: V) -> Option<(i32, Vec<u8>)> {
    let compound = tag.extract_compound()?;
    let name = compound
        .get_string("id")
        .map(|name| name.strip_prefix("minecraft:").unwrap_or(name).to_owned())
        .or_else(|| {
            compound
                .get("Id")
                .and_then(extract_int_like)
                .and_then(|id| registry_entry_name(version, "mob_effect", id))
                .map(ToOwned::to_owned)
        })?;
    let id = registry_entry_id(V::V_26_3, "mob_effect", &name)?;
    Some((id, potion_effect_data_from_nbt(compound)?))
}

fn potion_effect_data_from_nbt(compound: &NbtCompound) -> Option<Vec<u8>> {
    let amplifier = compound
        .get("Amplifier")
        .or_else(|| compound.get("amplifier"))
        .and_then(extract_int_like)
        .unwrap_or(0);
    let duration = compound
        .get("Duration")
        .or_else(|| compound.get("duration"))
        .and_then(extract_int_like)
        .unwrap_or(0);
    let flag = |upper: &str, lower: &str| {
        compound
            .get_bool(upper)
            .or_else(|| compound.get_bool(lower))
            .or_else(|| {
                compound
                    .get(upper)
                    .or_else(|| compound.get(lower))
                    .and_then(extract_int_like)
                    .map(|value| value != 0)
            })
            .unwrap_or(false)
    };
    let hidden = compound
        .get_compound("HiddenEffect")
        .or_else(|| compound.get_compound("hidden_effect"));
    let mut out = var_int(amplifier);
    out.extend(var_int(duration));
    out.extend([
        u8::from(flag("Ambient", "ambient")),
        u8::from(flag("ShowParticles", "show_particles")),
        u8::from(flag("ShowIcon", "show_icon")),
        u8::from(hidden.is_some()),
    ]);
    if let Some(hidden) = hidden {
        out.extend(potion_effect_data_from_nbt(hidden)?);
    }
    Some(out)
}

fn profile_from_skull_owner(owner: &NbtTag) -> Option<Profile> {
    let mut profile = Profile::default();
    match owner {
        NbtTag::String(name) => profile.name = Some(name.to_string()),
        NbtTag::Compound(compound) => {
            profile.name = compound.get_string("Name").map(ToOwned::to_owned);
            profile.id = compound
                .get_int_array("Id")
                .and_then(|id| <[i32; 4]>::try_from(id).ok());
            if let Some(textures) = compound
                .get_compound("Properties")
                .and_then(|properties| properties.get_list("textures"))
            {
                for texture in textures {
                    let Some(texture) = texture.extract_compound() else {
                        continue;
                    };
                    let Some(value) = texture.get_string("Value") else {
                        continue;
                    };
                    profile.properties.push((
                        "textures".to_owned(),
                        value.to_owned(),
                        texture.get_string("Signature").map(ToOwned::to_owned),
                    ));
                }
            }
        }
        _ => return None,
    }
    (profile.name.is_some() || profile.id.is_some() || !profile.properties.is_empty())
        .then_some(profile)
}

fn component(id: DataComponent, data: Vec<u8>) -> ItemComponent {
    ItemComponent {
        id: i32::from(id.to_id()),
        data,
    }
}

fn var_int(value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = out.write_var_int(&VarInt(value));
    out
}

/// The 26.3 components a client's item NBT stands for.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn nbt_to_components(nbt: &NbtCompound, version: V) -> Vec<ItemComponent> {
    let mut out = Vec::new();
    let mut consumed_instrument = false;
    let hide_flags = nbt.get("HideFlags").and_then(extract_int_like).unwrap_or(0);
    let mut translated_hide_flags = 0;
    let mut hidden_components = Vec::new();

    if let Some(damage) = nbt.get("Damage").and_then(extract_int_like) {
        out.push(component(DataComponent::Damage, var_int(damage)));
    }
    if let Some(cost) = nbt.get("RepairCost").and_then(extract_int_like) {
        out.push(component(DataComponent::RepairCost, var_int(cost)));
    }
    if nbt
        .get("Unbreakable")
        .and_then(extract_int_like)
        .is_some_and(|value| value != 0)
    {
        out.push(component(DataComponent::Unbreakable, Vec::new()));
    }
    if let Some(value) = nbt.get("CustomModelData").and_then(extract_int_like) {
        let mut data = var_int(1);
        data.extend((value as f32).to_bits().to_be_bytes());
        data.extend([0, 0, 0]);
        out.push(component(DataComponent::CustomModelData, data));
    }
    if let Some(value) = nbt.get("map").and_then(extract_int_like) {
        out.push(component(DataComponent::MapId, var_int(value)));
    }
    for (key, data_component, hide_flag) in [
        ("CanPlaceOn", DataComponent::CanPlaceOn, 16),
        ("CanDestroy", DataComponent::CanBreak, 8),
    ] {
        if let Some(list) = nbt.get_list(key)
            && let Some(data) = legacy_block_predicates_from_nbt(list)
        {
            out.push(component(data_component, data));
            if hide_flags & hide_flag != 0 {
                hidden_components.push(i32::from(data_component.to_id()));
                translated_hide_flags |= hide_flag;
            }
        }
    }
    for (tag, id) in [
        ("Enchantments", DataComponent::Enchantments),
        ("StoredEnchantments", DataComponent::StoredEnchantments),
    ] {
        let Some(list) = nbt.get_list(tag) else {
            continue;
        };
        let pairs = native_enchantment_list(list);
        if pairs.is_empty() {
            continue;
        }
        let mut data = var_int(i32::try_from(pairs.len()).unwrap_or(0));
        for (enchantment, level) in pairs {
            data.extend(var_int(enchantment));
            data.extend(var_int(level));
        }
        out.push(component(id, data));
    }
    let potion = nbt
        .get_string("Potion")
        .map(|name| name.strip_prefix("minecraft:").unwrap_or(name))
        .and_then(Potion::from_name)
        .map(|potion| i32::from(potion.id));
    let potion_color = nbt.get("CustomPotionColor").and_then(extract_int_like);
    let legacy_effects = nbt
        .get_list("CustomPotionEffects")
        .or_else(|| nbt.get_list("custom_potion_effects"))
        .map(|list| {
            list.iter()
                .filter_map(|effect| legacy_potion_effect_from_nbt(effect, version))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if potion.is_some() || potion_color.is_some() || !legacy_effects.is_empty() {
        let mut data = Vec::new();
        match potion {
            Some(id) => {
                data.push(1);
                data.extend(var_int(id));
            }
            None => data.push(0),
        }
        match potion_color {
            Some(color) => {
                data.push(1);
                data.extend(color.to_be_bytes());
            }
            None => data.push(0),
        }
        data.extend(var_int(i32::try_from(legacy_effects.len()).unwrap_or(0)));
        for (effect_id, effect_data) in legacy_effects {
            data.extend(var_int(effect_id));
            data.extend(effect_data);
        }
        data.push(0); // No custom potion name.
        out.push(component(DataComponent::PotionContents, data));
    }
    if let Some(profile) = nbt.get("SkullOwner").and_then(profile_from_skull_owner) {
        let mut data = Vec::new();
        if write_profile(&profile, &mut data).is_some() {
            out.push(component(DataComponent::Profile, data));
        }
    }
    if let Some(block_entity) = nbt.get_compound("BlockEntityTag") {
        let mut data = var_int(0);
        if data
            .write_nbt_with_version(Some(&NbtTag::Compound(block_entity.clone())), &V::V_26_2)
            .is_ok()
        {
            out.push(component(DataComponent::BlockEntityData, data));
        }
    }
    if let Some(trim) = nbt.get_compound("Trim")
        && let (Some(material), Some(pattern)) = (
            trim.get_string("material")
                .map(|name| name.strip_prefix("minecraft:").unwrap_or(name)),
            trim.get_string("pattern")
                .map(|name| name.strip_prefix("minecraft:").unwrap_or(name)),
        )
        && let (Some(material), Some(pattern)) = (
            registry_entry_id(V::V_26_3, "trim_material", material),
            registry_entry_id(V::V_26_3, "trim_pattern", pattern),
        )
    {
        let mut data = var_int(material + 1);
        data.extend(var_int(pattern + 1));
        out.push(component(DataComponent::Trim, data));
    }
    if let Some(instrument) = nbt.get_string("instrument") {
        let instrument = instrument.strip_prefix("minecraft:").unwrap_or(instrument);
        if let Some(registry_id) = registry_entry_id(V::V_26_3, "instrument", instrument)
            && let Some(holder_id) = registry_id.checked_add(1)
        {
            out.push(component(DataComponent::Instrument, var_int(holder_id)));
            consumed_instrument = true;
        }
    }
    let pages: Vec<String> = nbt
        .get_list("pages")
        .map(|pages| {
            pages
                .iter()
                .filter_map(NbtTag::extract_string)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if !pages.is_empty() {
        match (nbt.get_string("title"), nbt.get_string("author")) {
            (Some(title), Some(author)) => {
                let mut data = Vec::new();
                let _ = data.write_string(title);
                data.push(0);
                let _ = data.write_string(author);
                data.push(0);
                data.extend(var_int(i32::try_from(pages.len()).unwrap_or(0)));
                for page in &pages {
                    let _ = data.write_nbt_with_version(
                        Some(&NbtTag::String(page.clone().into())),
                        &V::V_26_2,
                    );
                    data.push(0);
                }
                data.push(1);
                out.push(component(DataComponent::WrittenBookContent, data));
            }
            _ => {
                let mut data = var_int(i32::try_from(pages.len()).unwrap_or(0));
                for page in &pages {
                    let _ = data.write_string(page);
                    data.push(0);
                }
                out.push(component(DataComponent::WritableBookContent, data));
            }
        }
    }
    if !hidden_components.is_empty() {
        let mut tooltip = vec![0]; // Do not hide every tooltip.
        tooltip.extend(var_int(i32::try_from(hidden_components.len()).unwrap_or(0)));
        for id in hidden_components {
            tooltip.extend(var_int(id));
        }
        out.push(component(DataComponent::TooltipDisplay, tooltip));
    }
    if let Some(display) = nbt.get_compound("display") {
        if let Some(name) = display.get_string("Name") {
            let mut data = Vec::new();
            if data
                .write_nbt_with_version(
                    Some(&text_from_json(name).to_nbt_tag_for_version(&V::V_26_2)),
                    &V::V_26_2,
                )
                .is_ok()
            {
                out.push(component(DataComponent::CustomName, data));
            }
        }
        if let Some(lore) = display.get_list("Lore") {
            let lines: Vec<TextComponent> = lore
                .iter()
                .filter_map(NbtTag::extract_string)
                .map(|line| lore_from_wire(line, version))
                .collect();
            if !lines.is_empty() {
                let mut data = var_int(i32::try_from(lines.len()).unwrap_or(0));
                for line in &lines {
                    let _ = data.write_nbt_with_version(
                        Some(&line.to_nbt_tag_for_version(&V::V_26_2)),
                        &V::V_26_2,
                    );
                }
                out.push(component(DataComponent::Lore, data));
            }
        }
        if let Some(color) = display.get("color").and_then(extract_int_like) {
            out.push(component(
                DataComponent::DyedColor,
                color.to_be_bytes().to_vec(),
            ));
        }
    }

    // Everything else the client sent, so the stack is the same stack when it
    // comes back; `AttributeModifiers` is here because a 26.3 modifier is
    // identified by a resource id a string off the wire cannot become.
    let mut custom = NbtCompound::new();
    for (name, tag) in &nbt.child_tags {
        if !CONSUMED_ROOT_TAGS.contains(&name.as_ref())
            && !(consumed_instrument && name.as_ref() == "instrument")
        {
            custom.put(name, tag.clone());
        }
    }
    let unhandled_hide_flags = hide_flags & !translated_hide_flags;
    if unhandled_hide_flags != 0 {
        custom.put_int("HideFlags", unhandled_hide_flags);
    }
    if let Some(display) = nbt.get_compound("display") {
        let mut custom_display = display.clone();
        for name in ["Name", "Lore", "color"] {
            custom_display.child_tags.remove(name);
        }
        if !custom_display.is_empty() {
            custom.put_compound("display", custom_display);
        }
    }
    if !custom.is_empty() {
        let mut data = Vec::new();
        if data
            .write_nbt_with_version(Some(&NbtTag::Compound(custom)), &V::V_26_2)
            .is_ok()
        {
            out.push(component(DataComponent::CustomData, data));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> &'static ComposedMappings {
        crate::api::MappingData::get().composed(V::V_1_16_2)
    }

    #[test]
    fn sweeping_is_renamed_in_both_directions() {
        let sweeping = Enchantment::from_name(NATIVE_SWEEPING).unwrap();
        assert_eq!(
            legacy_enchantment_name(sweeping, V::V_1_20),
            Some(LEGACY_SWEEPING)
        );
        assert_eq!(
            native_enchantment("minecraft:sweeping").map(|e| e.registry_key),
            Some(NATIVE_SWEEPING)
        );
    }

    /// `breach` arrives in 1.21, so no client in the NBT range has the name.
    #[test]
    fn enchantments_the_client_lacks_are_dropped() {
        let breach = Enchantment::from_name("breach").unwrap();
        assert_eq!(legacy_enchantment_name(breach, V::V_1_20), None);
        let sharpness = Enchantment::from_name("sharpness").unwrap();
        assert_eq!(
            legacy_enchantment_name(sharpness, V::V_1_16_2),
            Some("sharpness")
        );
    }

    #[test]
    fn swift_sneak_starts_at_1_19() {
        let swift_sneak = Enchantment::from_name("swift_sneak").unwrap();
        assert_eq!(legacy_enchantment_name(swift_sneak, V::V_1_18_2), None);
        assert_eq!(
            legacy_enchantment_name(swift_sneak, V::V_1_19),
            Some("swift_sneak")
        );
    }

    #[test]
    fn attribute_names_are_the_version_s_own() {
        assert_eq!(
            legacy_attribute_name(i32::from(Attributes::MAX_HEALTH.id), V::V_1_16_2),
            Some("generic.max_health")
        );
        assert_eq!(
            legacy_attribute_name(i32::from(Attributes::JUMP_STRENGTH.id), V::V_1_20),
            Some("horse.jump_strength")
        );
        assert_eq!(
            legacy_attribute_name(i32::from(Attributes::MAX_ABSORPTION.id), V::V_1_20),
            None
        );
        assert_eq!(
            legacy_attribute_name(i32::from(Attributes::MAX_ABSORPTION.id), V::V_1_20_2),
            Some("generic.max_absorption")
        );
    }

    #[test]
    fn potions_added_in_1_21_have_no_name_here() {
        let oozing = Potion::from_name("oozing").unwrap();
        assert_eq!(legacy_potion_name(i32::from(oozing.id), V::V_1_20), None);
        let healing = Potion::from_name("healing").unwrap();
        assert_eq!(
            legacy_potion_name(i32::from(healing.id), V::V_1_20),
            Some("minecraft:healing".to_owned())
        );
    }

    #[test]
    fn unknown_tags_round_trip_through_custom_data() {
        let mut nbt = NbtCompound::new();
        nbt.put_int("HideFlags", 63);
        nbt.put_int("Damage", 3);

        let added = nbt_to_components(&nbt, V::V_1_16_2);
        let back = components_to_nbt(&added, V::V_1_16_2, ids()).unwrap();
        assert_eq!(back.get("HideFlags").and_then(extract_int_like), Some(63));
        assert_eq!(back.get_int("Damage"), Some(3));
    }

    #[test]
    fn adventure_predicates_round_trip_with_legacy_hide_flags() {
        let mut nbt = NbtCompound::new();
        nbt.put_list(
            "CanPlaceOn",
            vec![NbtTag::String("minecraft:stone[axis=y]".into())],
        );
        nbt.put_list(
            "CanDestroy",
            vec![NbtTag::String("#minecraft:mineable/pickaxe".into())],
        );
        // The component-backed bits (8 and 16) become TooltipDisplay, while
        // unrelated legacy HideFlags stay in CustomData.
        nbt.put_byte("HideFlags", 63);

        let components = nbt_to_components(&nbt, V::V_1_16_2);
        assert!(
            components
                .iter()
                .any(|entry| { entry.id == i32::from(DataComponent::CanPlaceOn.to_id()) })
        );
        assert!(
            components
                .iter()
                .any(|entry| { entry.id == i32::from(DataComponent::CanBreak.to_id()) })
        );
        let tooltip = components
            .iter()
            .find(|entry| entry.id == i32::from(DataComponent::TooltipDisplay.to_id()))
            .expect("legacy HideFlags become per-component tooltip visibility");
        let mut tooltip_data = tooltip.data.as_slice();
        assert!(!tooltip_data.get_bool().unwrap());
        assert_eq!(tooltip_data.get_var_int().unwrap().0, 2);
        let hidden = [
            tooltip_data.get_var_int().unwrap().0,
            tooltip_data.get_var_int().unwrap().0,
        ];
        assert!(hidden.contains(&i32::from(DataComponent::CanPlaceOn.to_id())));
        assert!(hidden.contains(&i32::from(DataComponent::CanBreak.to_id())));

        let back = components_to_nbt(&components, V::V_1_16_2, ids()).unwrap();
        assert_eq!(back.get("HideFlags").and_then(extract_int_like), Some(63));
        assert_eq!(
            back.get_list("CanPlaceOn").unwrap()[0].extract_string(),
            Some("minecraft:stone[axis=y]")
        );
        assert_eq!(
            back.get_list("CanDestroy").unwrap()[0].extract_string(),
            Some("#minecraft:mineable/pickaxe")
        );
    }

    #[test]
    fn legacy_adventure_ranges_are_dropped_instead_of_widened() {
        let stone = registry_entry_id(V::V_26_3, "block", "stone").unwrap();
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(1)).unwrap(); // one predicate
        payload.write_bool(true).unwrap(); // holder set present
        payload.write_var_int(&VarInt(2)).unwrap(); // one explicit block id
        payload.write_var_int(&VarInt(stone)).unwrap();
        payload.write_bool(true).unwrap(); // property matchers present
        payload.write_var_int(&VarInt(1)).unwrap(); // one property
        payload.write_string("axis").unwrap();
        payload.write_bool(false).unwrap(); // range, not exact equality
        payload.write_bool(true).unwrap();
        payload.write_string("x").unwrap();
        payload.write_bool(false).unwrap(); // no upper bound
        payload.write_bool(false).unwrap(); // no NBT matcher
        payload.write_var_int(&VarInt(0)).unwrap(); // no component matchers
        payload.write_var_int(&VarInt(0)).unwrap(); // no item matchers

        let rewritten = legacy_block_predicates(&payload, V::V_1_16_2, ids()).unwrap();
        assert!(
            rewritten.is_empty(),
            "dropping an unrepresentable range must not allow every state of the block"
        );
    }

    #[test]
    fn custom_potion_effects_round_trip_through_legacy_nbt() {
        let source_effect = registry_entry_id(V::V_26_3, "mob_effect", "speed").unwrap();
        let target_effect = registry_entry_id(V::V_1_16_2, "mob_effect", "speed").unwrap();
        let mut data = vec![0, 0]; // no base potion or custom color
        data.extend(var_int(1)); // one custom effect
        data.extend(var_int(source_effect));
        data.extend(var_int(1)); // amplifier
        data.extend(var_int(1200)); // duration
        data.extend([0, 1, 1, 0]); // ambient, particles, icon, no hidden effect
        data.push(0); // no custom potion name

        let nbt = components_to_nbt(
            &[component(DataComponent::PotionContents, data.clone())],
            V::V_1_16_2,
            ids(),
        )
        .unwrap();
        let effect = nbt.get_list("CustomPotionEffects").unwrap()[0]
            .extract_compound()
            .unwrap();
        assert_eq!(
            effect.get("Id").and_then(extract_int_like),
            Some(target_effect)
        );
        let restored = nbt_to_components(&nbt, V::V_1_16_2);
        assert_eq!(
            restored
                .iter()
                .find(|entry| entry.id == i32::from(DataComponent::PotionContents.to_id()))
                .map(|entry| entry.data.as_slice()),
            Some(data.as_slice())
        );
    }

    #[test]
    fn charged_projectiles_become_legacy_crossbow_nbt_items() {
        let version = V::V_1_16_2;
        let arrow = i32::from(pumpkin_data::item::Item::ARROW.id);
        let mut nested = Vec::new();
        TEMPLATE_ITEM
            .write(
                &mut nested,
                &Item::Structured {
                    count: 1,
                    id: arrow,
                    added: Vec::new(),
                    removed: Vec::new(),
                },
            )
            .unwrap();
        let mut payload = var_int(1);
        payload.extend(nested);
        let nbt = components_to_nbt(
            &[component(DataComponent::ChargedProjectiles, payload)],
            version,
            ids(),
        )
        .unwrap();
        assert!(nbt.get_bool("Charged").unwrap_or(false));
        let projectile = nbt.get_list("ChargedProjectiles").unwrap()[0]
            .extract_compound()
            .unwrap();
        assert_eq!(
            projectile.get_string("id").as_deref(),
            Some("minecraft:arrow")
        );
        assert_eq!(projectile.get_byte("Count"), Some(1));
    }

    #[test]
    fn unknown_display_children_survive_legacy_component_round_trips() {
        let version = V::V_1_16_2;
        let mut display = NbtCompound::new();
        display.put_string("Name", r#"{"text":"Display name"}"#.to_owned());
        display.put_list(
            "Lore",
            vec![NbtTag::String(r#"{"text":"Lore line"}"#.into())],
        );
        display.put_int("color", 0x12_3456);
        display.put_int("plugin_marker", 99);
        let mut nbt = NbtCompound::new();
        nbt.put_compound("display", display);

        let components = nbt_to_components(&nbt, version);
        let back = components_to_nbt(&components, version, ids()).unwrap();
        let display = back.get_compound("display").unwrap();
        assert_eq!(display.get_int("plugin_marker"), Some(99));
        assert!(display.get_string("Name").is_some());
        assert!(display.get_list("Lore").is_some());
        assert_eq!(display.get_int("color"), Some(0x12_3456));
    }

    #[test]
    fn enchantments_round_trip_through_the_nbt_form() {
        let sharpness = Enchantment::from_name("sharpness").unwrap();
        let mut data = var_int(1);
        data.extend(var_int(i32::from(sharpness.id)));
        data.extend(var_int(4));
        let added = vec![component(DataComponent::Enchantments, data)];

        let nbt = components_to_nbt(&added, V::V_1_16_2, ids()).unwrap();
        let entry = nbt.get_list("Enchantments").unwrap()[0]
            .extract_compound()
            .unwrap();
        assert_eq!(entry.get_string("id"), Some("minecraft:sharpness"));
        assert_eq!(entry.get_short("lvl"), Some(4));

        let back = nbt_to_components(&nbt, V::V_1_16_2);
        let enchantments = find(&back, DataComponent::Enchantments).unwrap();
        assert_eq!(
            var_int_pairs(enchantments).unwrap(),
            vec![(i32::from(sharpness.id), 4)]
        );
    }

    #[test]
    fn enchantments_missing_from_1_20_3_use_vias_display_name_lore() {
        let density = Enchantment::from_name("density").unwrap();
        let mut data = var_int(1);
        data.extend(var_int(i32::from(density.id)));
        data.extend(var_int(3));
        let added = vec![component(DataComponent::Enchantments, data)];
        let version = V::V_1_20_3;

        let nbt = components_to_nbt(&added, version, MappingData::get().composed(version))
            .expect("the unsupported enchantment still has display lore");
        assert!(nbt.get_list("Enchantments").is_none());
        let lore = nbt
            .get_compound("display")
            .and_then(|display| display.get_list("Lore"))
            .expect("fallback enchantment lore");
        assert!(lore[0].extract_string().unwrap().contains("Density III"));
    }

    #[test]
    fn armor_trim_registry_values_round_trip_through_legacy_nbt() {
        let material = registry_entry_id(V::V_26_3, "trim_material", "iron").unwrap();
        let pattern = registry_entry_id(V::V_26_3, "trim_pattern", "coast").unwrap();
        let mut trim_data = var_int(material + 1);
        trim_data.extend(var_int(pattern + 1));
        let added = vec![component(DataComponent::Trim, trim_data)];

        let nbt = components_to_nbt(&added, V::V_1_20_3, ids()).unwrap();
        let trim = nbt.get_compound("Trim").unwrap();
        assert_eq!(trim.get_string("material"), Some("minecraft:iron"));
        assert_eq!(trim.get_string("pattern"), Some("minecraft:coast"));

        let back = nbt_to_components(&nbt, V::V_1_20_3);
        let data = find(&back, DataComponent::Trim).unwrap();
        let mut read = data;
        assert_eq!(read.get_var_int().unwrap().0, material + 1);
        assert_eq!(read.get_var_int().unwrap().0, pattern + 1);
        assert!(read.is_empty());
    }

    #[test]
    fn goat_horn_instrument_round_trips_through_legacy_nbt() {
        let registry_id = registry_entry_id(V::V_26_3, "instrument", "ponder_goat_horn").unwrap();
        let holder_id = registry_id + 1;
        let component = component(DataComponent::Instrument, var_int(holder_id));

        let nbt = components_to_nbt(&[component], V::V_1_19, ids()).unwrap();
        assert_eq!(
            nbt.get_string("instrument"),
            Some("minecraft:ponder_goat_horn")
        );

        let back = nbt_to_components(&nbt, V::V_1_19);
        let instrument = find(&back, DataComponent::Instrument).unwrap();
        assert_eq!(instrument, var_int(holder_id).as_slice());
        let custom_data = find(&back, DataComponent::CustomData);
        assert!(
            custom_data.is_none(),
            "recognized instrument is not custom data"
        );
    }

    #[test]
    fn a_group_slot_modifier_is_left_out() {
        let modifier = Modifier {
            attribute: i32::from(Attributes::ATTACK_DAMAGE.id),
            id: "minecraft:base_attack_damage".to_owned(),
            amount: 3.0,
            operation: 0,
            // The armour group slot.
            slot: 8,
        };
        assert!(legacy_attribute_entry(&modifier, 0, V::V_1_20).is_none());
    }

    /// 1.16 replaced the two UUID longs with an int array.
    #[test]
    fn an_attribute_entry_has_the_1_16_uuid_array() {
        let modifier = Modifier {
            attribute: i32::from(Attributes::ATTACK_DAMAGE.id),
            id: "minecraft:base_attack_damage".to_owned(),
            amount: 3.0,
            operation: 1,
            slot: 1,
        };
        let entry = legacy_attribute_entry(&modifier, 0, V::V_1_16_2).unwrap();
        assert_eq!(
            entry.get_string("AttributeName"),
            Some("generic.attack_damage")
        );
        assert_eq!(entry.get_string("Slot"), Some("mainhand"));
        assert_eq!(entry.get_int("Operation"), Some(1));
        assert_eq!(entry.get_int_array("UUID").map(<[i32]>::len), Some(4));
    }

    /// The 26.3 payload is a list of floats, flags, strings and colours; only
    /// a single whole float can become the old int tag.
    #[test]
    fn custom_model_data_needs_a_single_whole_float() {
        let mut single = var_int(1);
        single.extend(7.0f32.to_bits().to_be_bytes());
        single.extend([0, 0, 0]);
        assert_eq!(legacy_custom_model_data(&single), Some(7));

        let mut fractional = var_int(1);
        fractional.extend(7.5f32.to_bits().to_be_bytes());
        fractional.extend([0, 0, 0]);
        assert_eq!(legacy_custom_model_data(&fractional), None);
    }

    #[test]
    fn entity_hover_uses_the_via_backwards_standin_name() {
        use pumpkin_util::text::hover::HoverEvent;

        let mut component = TextComponent::text("entity");
        component.0.style.hover_event = Some(HoverEvent::show_entity(
            "00000000-0000-0000-0000-000000000001",
            "minecraft:nautilus",
            None,
        ));
        let version = V::V_1_21_9;
        let sanitized = sanitize_text(
            &component,
            version,
            crate::api::MappingData::get().composed(version),
        );

        let Some(HoverEvent::ShowEntity { id, .. }) = &sanitized.0.style.hover_event else {
            panic!("the mapped entity hover should remain visible")
        };
        assert_eq!(id.as_ref(), "minecraft:squid");
    }
}
