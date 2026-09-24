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

use crate::api::ComposedMappings;
use crate::api::rewriter::item_component::legacy_modifier_uuid;
use crate::api::types::ItemComponent;
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
            *id = mapped.resource_name.into();
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

fn legacy_enchantment_list(pairs: &[(i32, i32)], version: V) -> Vec<NbtTag> {
    let mut tags = Vec::with_capacity(pairs.len());
    for (id, level) in pairs {
        let Some(name) = u8::try_from(*id)
            .ok()
            .and_then(Enchantment::from_id)
            .and_then(|enchantment| legacy_enchantment_name(enchantment, version))
        else {
            continue;
        };
        let mut entry = NbtCompound::new();
        entry.put_string("id", format!("minecraft:{name}"));
        entry.put_short("lvl", (*level).clamp(0, i32::from(i16::MAX)) as i16);
        tags.push(NbtTag::Compound(entry));
    }
    tags
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
fn legacy_custom_model_data(bytes: &[u8]) -> Option<i32> {
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
    let mut display = NbtCompound::new();

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
                let list = legacy_enchantment_list(&pairs, version);
                if list.is_empty() {
                    continue;
                }
                if component == DataComponent::Enchantments {
                    root.put_list("Enchantments", list);
                } else {
                    root.put_list("StoredEnchantments", list);
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
                // Custom effects carry numeric effect ids the client numbers
                // differently, so they are left out rather than renumbered.
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
                // Armour trims arrive in 1.20, and core writes the payload as
                // two empty holders, so the tag can only be a placeholder.
                if version >= V::V_1_20 {
                    let mut trim = NbtCompound::new();
                    trim.put("material", NbtTag::String("minecraft:quartz".into()));
                    trim.put("pattern", NbtTag::String("minecraft:coast".into()));
                    root.put_compound("Trim", trim);
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

    if !display.is_empty() {
        root.put_compound("display", display);
    }
    if root.is_empty() { None } else { Some(root) }
}

/// The root tags this module turns into components; everything else a client
/// sends is kept in `minecraft:custom_data`.
static CONSUMED_ROOT_TAGS: &[&str] = &[
    "BlockEntityTag",
    "CustomModelData",
    "CustomPotionColor",
    "Damage",
    "Enchantments",
    "Potion",
    "RepairCost",
    "SkullOwner",
    "StoredEnchantments",
    "Trim",
    "Unbreakable",
    "author",
    "display",
    "map",
    "pages",
    "resolved",
    "title",
];

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
    if potion.is_some() || potion_color.is_some() {
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
        // No custom effects and no custom name.
        data.extend([0, 0]);
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
    if nbt.get_compound("Trim").is_some() {
        out.push(component(DataComponent::Trim, vec![0, 0]));
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
        if !CONSUMED_ROOT_TAGS.contains(&name.as_ref()) {
            custom.put(name, tag.clone());
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
        assert_eq!(back.get_int("HideFlags"), Some(63));
        assert_eq!(back.get_int("Damage"), Some(3));
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
