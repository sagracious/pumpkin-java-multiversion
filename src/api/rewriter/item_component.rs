use pumpkin_data::data_component::DataComponent;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt, ReadingError};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::ComposedMappings;
use crate::api::rewriter::item_shape::{self, ID_SET, NBT, SOUND, STR, Shape, VAR_INT};
use crate::api::types::{ItemT, TEMPLATE_ITEM, WireType};

/// Scratch buffers cannot run out of room, so the write errors of a byte
/// transform are reported as reading errors.
trait IntoReading<T> {
    fn r(self) -> Result<T, ReadingError>;
}

impl<T> IntoReading<T> for Result<T, pumpkin_protocol::ser::WritingError> {
    fn r(self) -> Result<T, ReadingError> {
        self.map_err(|error| ReadingError::Message(error.to_string()))
    }
}

/// The oldest version whose encoding of `component` is 26.3's.
///
/// Derived by diffing `SlotComponent` in `minecraft-data`'s `protocol.json`
/// from 1.20.5 to 1.21.11, plus the 26.x deltas core's own writer carries.
#[must_use]
pub fn shape_floor(component: DataComponent) -> V {
    use DataComponent as C;
    match component {
        C::AttributeModifiers | C::JukeboxPlayable | C::AttackAnimation | C::InteractAnimation => {
            V::V_26_2
        }
        C::EntityData | C::BlockEntityData | C::Profile => V::V_1_21_9,
        C::Equippable => V::V_1_21_6,
        C::Unbreakable
        | C::Enchantments
        | C::StoredEnchantments
        | C::DyedColor
        | C::Tool
        | C::CanPlaceOn
        | C::CanBreak
        | C::IntangibleProjectile => V::V_1_21_5,
        C::Trim | C::Instrument | C::ProvidesTrimMaterial => V::V_26_3,
        C::Food => V::V_1_21_2,
        // The four-array codec arrived in 1.21.4 and is unchanged in later
        // protocols. Only older layouts use the legacy integer payload.
        C::CustomModelData => V::V_1_21_4,
        _ => V::V_1_20_5,
    }
}

/// The 26.3 payload of `component` in `target`'s layout, or `None` when it
/// cannot be expressed there.
#[must_use]
pub fn to_version(
    component: DataComponent,
    native: &[u8],
    target: V,
    ids: &ComposedMappings,
) -> Option<Vec<u8>> {
    to_version_with_tooltip(component, native, target, ids, true)
}

/// The 26.3 payload of a component in `target`'s layout, carrying its
/// component-specific visibility from the 26.3 tooltip display.
#[must_use]
pub fn to_version_with_tooltip(
    component: DataComponent,
    native: &[u8],
    target: V,
    ids: &ComposedMappings,
    show_in_tooltip: bool,
) -> Option<Vec<u8>> {
    let mapped = map_nested_ids(component, native, target, ids).ok()?;
    if target >= V::V_26_3 {
        return Some(mapped);
    }
    if target >= shape_floor(component) {
        return Some(mapped);
    }
    adapt(component, mapped, target, show_in_tooltip)
        .ok()
        .flatten()
}

fn adapt(
    component: DataComponent,
    native: Vec<u8>,
    target: V,
    show_in_tooltip: bool,
) -> Result<Option<Vec<u8>>, ReadingError> {
    use DataComponent as C;
    let out = match component {
        C::Unbreakable => vec![1],
        C::Enchantments | C::StoredEnchantments | C::DyedColor => {
            let mut out = native;
            out.push(1);
            out
        }
        C::Tool => {
            let mut out = native;
            out.pop();
            out
        }
        C::JukeboxPlayable if target >= V::V_1_21 => {
            let mut cursor = native.as_slice();
            let id = cursor.get_var_int()?.0;
            if !cursor.is_empty() {
                return Err(ReadingError::Message(
                    "trailing bytes in jukebox-playable component".into(),
                ));
            }
            let holder_id = id
                .checked_add(1)
                .ok_or_else(|| ReadingError::Message("jukebox-song holder id overflow".into()))?;
            let mut out = Vec::with_capacity(native.len() + 2);
            out.push(1);
            out.write_var_int(&VarInt(holder_id)).r()?;
            if target <= V::V_1_21_4 {
                out.push(1);
            }
            out
        }
        C::AttributeModifiers => attribute_modifiers(&native, target)?,
        C::Trim => legacy_trim(&native, target)?,
        C::Instrument => legacy_instrument(&native, target)?,
        C::ProvidesTrimMaterial => legacy_provides_trim_material(&native, target)?,
        C::CustomModelData => {
            let Some(value) = super::item_nbt::legacy_custom_model_data(&native) else {
                return Ok(None);
            };
            let mut out = Vec::new();
            out.write_var_int(&VarInt(value)).r()?;
            out
        }
        C::Equippable => match equippable(&native, target)? {
            Some(out) => out,
            None => return Ok(None),
        },
        C::CanPlaceOn | C::CanBreak => adventure_mode_predicates(&native, show_in_tooltip)?,
        C::EntityData | C::BlockEntityData => {
            let mut cursor = native.as_slice();
            cursor.get_var_int()?;
            cursor.to_vec()
        }
        C::Profile => profile(&native)?,
        // intangible_projectile, custom_model_data, food and anything else
        // whose shape moved has no converter.
        _ => return Ok(None),
    };
    Ok(Some(out))
}

/// 26.3 entries are (attribute, name, amount, operation, slot, display).
/// 1.21.4 and below wrap the list in a tooltip flag and 1.20.5 identifies an
/// entry by a UUID in front of the name; 1.21.11 has one display for the
/// whole component.
fn attribute_modifiers(native: &[u8], target: V) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?;
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&count).r()?;
    for index in 0..count.0 {
        out.write_var_int(&cursor.get_var_int()?).r()?;
        let name = cursor.get_str()?;
        if target <= V::V_1_20_5 {
            out.write_uuid(&legacy_modifier_uuid(&name, index)).r()?;
        }
        out.write_string(&name).r()?;
        out.write_f64_be(cursor.get_f64_be()?).r()?;
        out.write_var_int(&cursor.get_var_int()?).r()?;
        let slot = cursor.get_var_int()?.0;
        // The saddle slot (10) arrives in 1.21.5; body is the nearest one.
        let slot = if slot == 10 && target <= V::V_1_21_4 {
            9
        } else {
            slot
        };
        out.write_var_int(&VarInt(slot)).r()?;
        if cursor.get_var_int()?.0 == 2 {
            cursor.get_nbt(&V::V_26_3)?;
        }
    }
    if target <= V::V_1_21_4 {
        out.push(1);
    } else if target == V::V_1_21_11 {
        out.write_var_int(&VarInt(0)).r()?;
    }
    Ok(out)
}

/// A stand-in for the modifier UUID 1.20.5 identifies an entry by: stable
/// across sends and distinct within one component, which is all it is for.
#[must_use]
pub fn legacy_modifier_uuid(name: &str, index: i32) -> uuid::Uuid {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    let low = hash
        .rotate_left(32)
        .wrapping_add(u64::from(index.unsigned_abs()).wrapping_mul(PRIME));
    uuid::Uuid::from_u64_pair(hash, low)
}

fn copy_sound_holder(cursor: &mut &[u8], out: &mut Vec<u8>) -> Result<(), ReadingError> {
    let id = cursor.get_var_int()?;
    out.write_var_int(&id).r()?;
    if id.0 == 0 {
        out.write_string(&cursor.get_str()?).r()?;
        let has_range = cursor.get_bool()?;
        out.write_bool(has_range).r()?;
        if has_range {
            out.write_f32_be(cursor.get_f32_be()?).r()?;
        }
    }
    Ok(())
}

fn copy_id_set(cursor: &mut &[u8], out: &mut Vec<u8>) -> Result<(), ReadingError> {
    let n = cursor.get_var_int()?;
    if n.0 < 0 {
        return Err(ReadingError::Message("negative id-set length".into()));
    }
    out.write_var_int(&n).r()?;
    if n.0 == 0 {
        out.write_string(&cursor.get_str()?).r()?;
    } else {
        for _ in 1..n.0 {
            out.write_var_int(&cursor.get_var_int()?).r()?;
        }
    }
    Ok(())
}

/// 26.3 is slot, sound, model, camera overlay, allowed entities, dispensable,
/// swappable, damageable, equip on interact, shearable and shearing sound;
/// every older layout is a prefix of it.
fn equippable(native: &[u8], target: V) -> Result<Option<Vec<u8>>, ReadingError> {
    let mut cursor = native;
    let mut out = Vec::with_capacity(native.len());
    let slot = cursor.get_var_int()?;
    if slot.0 == 7 && target <= V::V_1_21_4 {
        return Ok(None);
    }
    out.write_var_int(&slot).r()?;
    copy_sound_holder(&mut cursor, &mut out)?;
    for _ in 0..2 {
        let present = cursor.get_bool()?;
        out.write_bool(present).r()?;
        if present {
            out.write_string(&cursor.get_str()?).r()?;
        }
    }
    let has_entities = cursor.get_bool()?;
    out.write_bool(has_entities).r()?;
    if has_entities {
        copy_id_set(&mut cursor, &mut out)?;
    }
    let bools = if target <= V::V_1_21_4 { 3 } else { 4 };
    for _ in 0..bools {
        out.write_bool(cursor.get_bool()?).r()?;
    }
    Ok(Some(out))
}

/// Before 1.21.9 a profile is name, id and properties, without the kind
/// prefix and the skin patch 26.3 wraps it in.
fn profile(native: &[u8]) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let mut out = Vec::with_capacity(native.len());
    cursor.get_var_int()?;
    let has_name = cursor.get_bool()?;
    out.write_bool(has_name).r()?;
    if has_name {
        out.write_string(&cursor.get_str()?).r()?;
    }
    let has_id = cursor.get_bool()?;
    out.write_bool(has_id).r()?;
    if has_id {
        out.write_uuid(&cursor.get_uuid()?).r()?;
    }
    let count = cursor.get_var_int()?;
    out.write_var_int(&count).r()?;
    for _ in 0..count.0 {
        out.write_string(&cursor.get_str()?).r()?;
        out.write_string(&cursor.get_str()?).r()?;
        let signed = cursor.get_bool()?;
        out.write_bool(signed).r()?;
        if signed {
            out.write_string(&cursor.get_str()?).r()?;
        }
    }
    Ok(out)
}

/// Rewrites the registry ids nested in a 26.3 payload for `target`.
fn map_nested_ids(
    component: DataComponent,
    native: &[u8],
    target: V,
    ids: &ComposedMappings,
) -> Result<Vec<u8>, ReadingError> {
    use DataComponent as C;
    match component {
        C::Trim => map_holder_ids(
            V::V_26_3,
            target,
            native,
            &["trim_material", "trim_pattern"],
        ),
        C::Instrument => map_holder_ids(V::V_26_3, target, native, &["instrument"]),
        C::ProvidesTrimMaterial => map_holder_ids(V::V_26_3, target, native, &["trim_material"]),
        C::JukeboxPlayable if target < V::V_26_3 => {
            map_registry_value(V::V_26_3, target, native, "jukebox_song")
        }
        C::Enchantments | C::StoredEnchantments => enchantment_ids(native, ids),
        C::AttributeModifiers => attribute_ids(native, ids),
        C::MapDecorations if target < V::V_26_3 => map_decoration_types(native),
        C::Consumable | C::DeathProtection if target < V::V_26_3 => {
            consume_effect_payload(component, native, true, false)
        }
        C::EntityData => leading_id(native, &ids.entities),
        C::BlockEntityData => leading_id(native, &ids.blockentities),
        C::PaintingVariant => holder_id(native, &ids.paintings),
        C::CanPlaceOn | C::CanBreak => map_block_predicates(native, target, ids),
        C::ChargedProjectiles | C::BundleContents if target >= V::V_1_20_5 => {
            nested_stack_array(native, target, ids)
        }
        C::Container if target >= V::V_1_20_5 => nested_stacks(native, target, ids, true),
        C::UseRemainder | C::SulfurCubeContent if target >= V::V_1_20_5 => {
            nested_stack(native, target, ids)
        }
        _ => Ok(native.to_vec()),
    }
}

fn map_block_predicates(
    native: &[u8],
    _target: V,
    ids: &ComposedMappings,
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?.0;
    if !(0..=4096).contains(&count) {
        return Err(ReadingError::Message(
            "block predicate count out of bounds".into(),
        ));
    }
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&VarInt(count)).r()?;
    for _ in 0..count {
        let has_blocks = cursor.get_bool()?;
        out.write_bool(has_blocks).r()?;
        if has_blocks {
            map_block_id_set(&mut cursor, &mut out, &ids.blocks)?;
        }
        for shape in [
            Shape::Opt(&Shape::Array(&Shape::Seq(&[
                Shape::Str,
                Shape::BoolSwitch(&STR, &Shape::Seq(&[Shape::Opt(&STR), Shape::Opt(&STR)])),
            ]))),
            Shape::Opt(&NBT),
            Shape::Array(&Shape::Component),
            Shape::Array(&VAR_INT),
        ] {
            copy_shape(&shape, &mut cursor, &mut out)?;
        }
    }
    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in adventure predicates: {}",
            cursor.len()
        )));
    }
    Ok(out)
}

fn map_block_id_set(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    mapping: &crate::api::IdMapping,
) -> Result<(), ReadingError> {
    let selector = cursor.get_var_int()?.0;
    if selector < 0 || selector > 4097 {
        return Err(ReadingError::Message(
            "block holder set out of bounds".into(),
        ));
    }
    if selector == 0 {
        out.write_var_int(&VarInt(0)).r()?;
        out.write_string(&cursor.get_str()?).r()?;
        return Ok(());
    }

    let mut mapped = Vec::with_capacity((selector - 1) as usize);
    for _ in 0..selector - 1 {
        if let Some(id) = map(mapping, cursor.get_var_int()?.0) {
            mapped.push(id);
        }
    }
    out.write_var_int(&VarInt(
        i32::try_from(mapped.len())
            .map_err(|_| ReadingError::Message("block holder set too large".into()))?
            + 1,
    ))
    .r()?;
    for id in mapped {
        out.write_var_int(&VarInt(id)).r()?;
    }
    Ok(())
}

fn adventure_mode_predicates(
    native: &[u8],
    show_in_tooltip: bool,
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?.0;
    if !(0..=4096).contains(&count) {
        return Err(ReadingError::Message(
            "block predicate count out of bounds".into(),
        ));
    }
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&VarInt(count)).r()?;
    for _ in 0..count {
        for shape in [
            Shape::Opt(&ID_SET),
            Shape::Opt(&Shape::Array(&Shape::Seq(&[
                Shape::Str,
                Shape::BoolSwitch(&STR, &Shape::Seq(&[Shape::Opt(&STR), Shape::Opt(&STR)])),
            ]))),
            Shape::Opt(&NBT),
        ] {
            copy_shape(&shape, &mut cursor, &mut out)?;
        }
        // 1.21.5 added data-component matchers to adventure predicates. Via
        // deliberately drops them when it writes the 1.21.4 predicate form.
        item_shape::skip(&Shape::Array(&Shape::Component), &mut cursor)?;
        item_shape::skip(&Shape::Array(&VAR_INT), &mut cursor)?;
    }
    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in adventure predicates: {}",
            cursor.len()
        )));
    }
    out.write_bool(show_in_tooltip).r()?;
    Ok(out)
}

fn map_holder_ids(
    source: V,
    target: V,
    payload: &[u8],
    registry_ids: &[&str],
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = payload;
    let mut output = Vec::with_capacity(payload.len());

    for registry_id in registry_ids {
        let source_holder_id = cursor.get_var_int()?.0;
        let Some(source_id) = source_holder_id.checked_sub(1) else {
            return Err(ReadingError::Message(format!(
                "inline {registry_id} data cannot be represented by Pumpkin"
            )));
        };
        let source_name = registry_entry_name(source, registry_id, source_id).ok_or_else(|| {
            ReadingError::Message(format!(
                "unknown {source} {registry_id} holder id {source_holder_id}"
            ))
        })?;
        let target_id = registry_entry_id(target, registry_id, source_name)
            .and_then(|id| id.checked_add(1))
            .ok_or_else(|| {
                ReadingError::Message(format!("{target} has no {registry_id} entry {source_name}"))
            })?;
        output.write_var_int(&VarInt(target_id)).r()?;
    }

    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in registry-backed item component: {}",
            cursor.len()
        )));
    }
    Ok(output)
}

/// Maps a registry-backed component whose payload is a direct registry id.
fn map_registry_value(
    source: V,
    target: V,
    payload: &[u8],
    registry_id: &str,
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = payload;
    let source_id = cursor.get_var_int()?.0;
    let source_name = registry_entry_name(source, registry_id, source_id).ok_or_else(|| {
        ReadingError::Message(format!("unknown {source} {registry_id} id {source_id}"))
    })?;
    let target_id = registry_entry_id(target, registry_id, source_name).ok_or_else(|| {
        ReadingError::Message(format!("{target} has no {registry_id} entry {source_name}"))
    })?;
    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in registry-backed item component: {}",
            cursor.len()
        )));
    }
    let mut out = Vec::new();
    out.write_var_int(&VarInt(target_id)).r()?;
    Ok(out)
}

/// Rewrites registry ids in a client item component to Pumpkin's 26.3 ids.
/// Components whose codecs have no nested registry ids pass through unchanged.
pub fn registry_ids_to_native(
    component: DataComponent,
    client: &[u8],
    version: V,
) -> Result<Vec<u8>, ReadingError> {
    use DataComponent as C;
    let registries = match component {
        C::Trim => &["trim_material", "trim_pattern"][..],
        C::Instrument => &["instrument"][..],
        C::ProvidesTrimMaterial => &["trim_material"][..],
        C::JukeboxPlayable => {
            return map_registry_value(version, V::V_26_3, client, "jukebox_song");
        }
        _ => return Ok(client.to_vec()),
    };
    map_holder_ids(version, V::V_26_3, client, registries)
}

/// Converts older jukebox-playable layouts to the canonical direct registry id.
/// Inline datapack songs are consumed but left unrepresentable; when the item
/// came from the server, the per-connection backup restores its source value.
pub(crate) fn legacy_jukebox_playable_to_native(
    payload: &mut &[u8],
    version: V,
    payload_is_bounded: bool,
) -> Result<Option<Vec<u8>>, ReadingError> {
    let id = if payload.get_bool()? {
        let holder_id = payload.get_var_int()?.0;
        if holder_id == 0 {
            item_shape::skip(&Shape::Sound, payload)?;
            payload.get_nbt(&version)?;
            payload.get_f32_be()?;
            payload.get_var_int()?;
            None
        } else {
            let source_id = holder_id
                .checked_sub(1)
                .ok_or_else(|| ReadingError::Message("invalid jukebox-song holder id".into()))?;
            let name =
                registry_entry_name(version, "jukebox_song", source_id).ok_or_else(|| {
                    ReadingError::Message(format!(
                        "unknown {version} jukebox_song holder id {holder_id}"
                    ))
                })?;
            registry_entry_id(V::V_26_3, "jukebox_song", name)
        }
    } else {
        let name = payload.get_str()?;
        let bare = name.strip_prefix("minecraft:").unwrap_or(&name);
        registry_entry_id(V::V_26_3, "jukebox_song", bare)
    };
    if version <= V::V_1_21_4 {
        payload.get_bool()?;
    }
    if payload_is_bounded && !payload.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in jukebox-playable component: {}",
            payload.len()
        )));
    }
    let Some(id) = id else {
        return Ok(None);
    };
    let mut out = Vec::new();
    out.write_var_int(&VarInt(id)).r()?;
    Ok(Some(out))
}

pub(crate) fn registry_entry_name(version: V, registry_id: &str, id: i32) -> Option<&'static str> {
    let id = usize::try_from(id).ok()?;
    if version >= V::V_26_3 {
        let registry = pumpkin_data::registry::REGISTRY_V_26_3
            .iter()
            .find(|registry| registry.registry_id == registry_id)?;
        return Some(registry.entries.get(id)?.name);
    }
    let registry = crate::registry::generated::get_synced(version)?
        .iter()
        .find(|registry| registry.registry_id == registry_id)?;
    Some(registry.entries.get(id)?.name)
}

pub(crate) fn registry_entry_id(version: V, registry_id: &str, name: &str) -> Option<i32> {
    let index = if version >= V::V_26_3 {
        let registry = pumpkin_data::registry::REGISTRY_V_26_3
            .iter()
            .find(|registry| registry.registry_id == registry_id)?;
        registry
            .entries
            .iter()
            .position(|entry| entry.name == name)?
    } else {
        let registry = crate::registry::generated::get_synced(version)?
            .iter()
            .find(|registry| registry.registry_id == registry_id)?;
        registry
            .entries
            .iter()
            .position(|entry| entry.name == name)?
    };
    i32::try_from(index).ok()
}

/// The 1.21.4-and-older trim component uses two holders and a tooltip flag.
/// `map_nested_ids` has already changed the canonical ids into this target's
/// registry numbering.
fn legacy_trim(native: &[u8], target: V) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let mut out = Vec::with_capacity(native.len() + 3);
    for _ in 0..2 {
        let holder = cursor.get_var_int()?.0;
        if holder <= 0 {
            return Err(ReadingError::Message("invalid trim holder id".into()));
        }
        out.write_var_int(&VarInt(holder)).r()?;
    }
    if !cursor.is_empty() {
        return Err(ReadingError::Message(
            "trailing bytes in trim component".into(),
        ));
    }
    if target <= V::V_1_21_4 {
        out.push(1); // show trim in tooltip
    }
    Ok(out)
}

/// The older Instrument component is a registry holder; 26.3 stores the
/// vanilla registry entry directly as one VarInt.
fn legacy_instrument(native: &[u8], target: V) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let holder = cursor.get_var_int()?.0;
    if !cursor.is_empty() {
        return Err(ReadingError::Message(
            "trailing bytes in instrument component".into(),
        ));
    }
    let mut out = Vec::with_capacity(native.len() + 2);
    if target == V::V_1_21_5 {
        out.push(1); // the holder arm of the 1.21.5 EitherHolder
    }
    out.write_var_int(&VarInt(holder)).r()?;
    Ok(out)
}

fn legacy_provides_trim_material(native: &[u8], target: V) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let holder = cursor.get_var_int()?.0;
    if holder <= 0 || !cursor.is_empty() {
        return Err(ReadingError::Message(
            "invalid provides-trim-material holder".into(),
        ));
    }
    let mut out = Vec::with_capacity(native.len() + 1);
    if target == V::V_1_21_5 {
        out.push(1); // the holder arm of the 1.21.5 EitherHolder
    }
    out.write_var_int(&VarInt(holder)).r()?;
    Ok(out)
}

/// Converts an older holder layout back to the canonical 26.3 holder ids.
/// Inline datapack values cannot be represented by Pumpkin's static models.
pub(crate) fn legacy_registry_component_to_native(
    component: DataComponent,
    payload: &mut &[u8],
    version: V,
    payload_is_bounded: bool,
) -> Result<Option<Vec<u8>>, ReadingError> {
    use DataComponent as C;
    let registries: &[&str] = match component {
        C::Trim => &["trim_material", "trim_pattern"],
        C::Instrument => &["instrument"],
        C::ProvidesTrimMaterial => &["trim_material"],
        _ => {
            return Err(ReadingError::Message(
                "not a legacy registry component".into(),
            ));
        }
    };

    let mut ids = Vec::with_capacity(registries.len());
    let mut representable = true;
    for registry in registries {
        let either_holder =
            version == V::V_1_21_5 && matches!(component, C::Instrument | C::ProvidesTrimMaterial);
        if either_holder && !payload.get_bool()? {
            let name = payload.get_str()?;
            let bare = name.strip_prefix("minecraft:").unwrap_or(&name);
            let native_id = registry_entry_id(V::V_26_3, registry, bare).ok_or_else(|| {
                ReadingError::Message(format!("26.3 has no {registry} entry {name}"))
            })?;
            ids.push(native_id.checked_add(1).ok_or_else(|| {
                ReadingError::Message(format!("26.3 {registry} holder id overflow"))
            })?);
            continue;
        }
        let holder_id = payload.get_var_int()?.0;
        if holder_id == 0 {
            representable = false;
            if component == C::Instrument {
                skip_inline_instrument(payload, version, false)?;
            } else {
                skip_inline_registry_value(payload, registry, version)?;
            }
            continue;
        }
        let source_id = holder_id.checked_sub(1).ok_or_else(|| {
            ReadingError::Message(format!(
                "invalid {version} {registry} holder id {holder_id}"
            ))
        })?;
        let name = registry_entry_name(version, registry, source_id).ok_or_else(|| {
            ReadingError::Message(format!(
                "unknown {version} {registry} holder id {holder_id}"
            ))
        })?;
        let native_id = registry_entry_id(V::V_26_3, registry, name)
            .and_then(|id| id.checked_add(1))
            .ok_or_else(|| ReadingError::Message(format!("26.3 has no {registry} entry {name}")))?;
        ids.push(native_id);
    }
    if component == C::Trim && version <= V::V_1_21_4 {
        payload.get_bool()?;
    }
    // Unprefixed component payloads share the remainder of the item packet;
    // only a length-delimited entry can be required to end here.
    if payload_is_bounded && !payload.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in component {}: {}",
            i32::from(component.to_id()),
            payload.len()
        )));
    }
    if !representable {
        return Ok(None);
    }

    let mut out = Vec::new();
    for id in ids {
        out.write_var_int(&VarInt(id)).r()?;
    }
    Ok(Some(out))
}

fn skip_inline_registry_value(
    payload: &mut &[u8],
    registry: &str,
    version: V,
) -> Result<(), ReadingError> {
    payload.get_str()?;
    if registry == "trim_pattern" {
        if version < V::V_1_21_5 {
            payload.get_var_int()?; // item id
        }
        payload.get_nbt(&version)?;
        payload.get_bool()?; // decal
        return Ok(());
    }
    if version < V::V_1_21_5 {
        payload.get_var_int()?; // item id
    }
    if version <= V::V_1_21_2 {
        payload.get_f32_be()?; // item model index
    }
    let count = payload.get_var_int()?.0;
    if !(0..=4096).contains(&count) {
        return Err(ReadingError::Message(
            "registry override count out of bounds".into(),
        ));
    }
    for _ in 0..count {
        if version <= V::V_1_21 {
            payload.get_var_int()?; // numeric armor-material id
        } else {
            payload.get_str()?;
        }
        payload.get_str()?;
    }
    payload.get_nbt(&version)?;
    Ok(())
}

fn skip_inline_instrument(
    payload: &mut &[u8],
    version: V,
    has_durability_damage: bool,
) -> Result<(), ReadingError> {
    if payload.get_var_int()?.0 == 0 {
        payload.get_str()?;
        if payload.get_bool()? {
            payload.get_f32_be()?;
        }
    }
    payload.get_f32_be()?;
    payload.get_f32_be()?;
    if has_durability_damage {
        payload.get_var_int()?;
    }
    payload.get_nbt(&version)?;
    Ok(())
}

/// ViaBackwards downgrades the five 26.3-only map decoration types to the
/// nearest 26.2 type. The item backup cache retains the original compound for
/// a matching 26.2 item returned by the client.
fn map_decoration_types(native: &[u8]) -> Result<Vec<u8>, ReadingError> {
    const NEW_TYPES: [&str; 5] = [
        "abandoned_camp",
        "ancient_city",
        "desert_pyramid",
        "mineshaft",
        "ocean_ruin_warm",
    ];

    let mut cursor = native;
    let Some(NbtTag::Compound(mut decorations)) = cursor.get_nbt(&V::V_26_3)? else {
        return Err(ReadingError::Message(
            "map decorations component is not an NBT compound".into(),
        ));
    };
    if !cursor.is_empty() {
        return Err(ReadingError::Message(
            "trailing bytes in map decorations component".into(),
        ));
    }

    for value in decorations.child_tags.values_mut() {
        let NbtTag::Compound(decoration) = value else {
            continue;
        };
        let Some(NbtTag::String(decoration_type)) = decoration.child_tags.get_mut("type") else {
            continue;
        };
        if NEW_TYPES.contains(&decoration_type.as_ref()) {
            *decoration_type = "village_plains".into();
        }
    }

    let mut out = Vec::with_capacity(native.len());
    out.write_nbt_with_version(Some(&NbtTag::Compound(decorations)), &V::V_26_3)
        .r()?;
    Ok(out)
}

/// Enchantment ids the target has no row for take the whole entry with them.
fn enchantment_ids(native: &[u8], ids: &ComposedMappings) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?.0;
    let mut kept: Vec<(VarInt, VarInt)> = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        let id = cursor.get_var_int()?;
        let level = cursor.get_var_int()?;
        if let Some(mapped) = map(&ids.enchantments, id.0) {
            kept.push((VarInt(mapped), level));
        }
    }
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&VarInt(i32::try_from(kept.len()).unwrap_or(0)))
        .r()?;
    for (id, level) in kept {
        out.write_var_int(&id).r()?;
        out.write_var_int(&level).r()?;
    }
    Ok(out)
}

fn attribute_ids(native: &[u8], ids: &ComposedMappings) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?.0;
    let mut kept = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        let attribute = cursor.get_var_int()?.0;
        let mut entry = Vec::new();
        let name = cursor.get_str()?;
        entry.write_string(&name).r()?;
        entry.write_f64_be(cursor.get_f64_be()?).r()?;
        entry.write_var_int(&cursor.get_var_int()?).r()?;
        entry.write_var_int(&cursor.get_var_int()?).r()?;
        let display = cursor.get_var_int()?;
        entry.write_var_int(&display).r()?;
        if display.0 == 2 {
            let tag = cursor.get_nbt(&V::V_26_3)?;
            entry.write_nbt_with_version(tag.as_ref(), &V::V_26_3).r()?;
        }
        if let Some(mapped) = map(&ids.attributes, attribute) {
            kept.push((mapped, entry));
        }
    }
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&VarInt(i32::try_from(kept.len()).unwrap_or(0)))
        .r()?;
    for (attribute, entry) in kept {
        out.write_var_int(&VarInt(attribute)).r()?;
        out.write_slice(&entry).r()?;
    }
    Ok(out)
}

fn leading_id(native: &[u8], mapping: &crate::api::IdMapping) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let id = cursor.get_var_int()?.0;
    let mapped = map(mapping, id).unwrap_or(0);
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&VarInt(mapped)).r()?;
    out.write_slice(cursor).r()?;
    Ok(out)
}

/// A registry holder: 0 is inline data, anything else is the id plus one.
fn holder_id(native: &[u8], mapping: &crate::api::IdMapping) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let raw = cursor.get_var_int()?.0;
    if raw == 0 {
        return Ok(native.to_vec());
    }
    let source_id = raw
        .checked_sub(1)
        .ok_or_else(|| ReadingError::Message("invalid registry holder id".into()))?;
    let mapped = map(mapping, source_id)
        .and_then(|id| id.checked_add(1))
        .ok_or_else(|| ReadingError::Message("unmapped registry holder id".into()))?;
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&VarInt(mapped)).r()?;
    out.write_slice(cursor).r()?;
    Ok(out)
}

/// `bundle_contents` is a plain list of stacks; `container` prefixes each
/// entry with a present flag.
fn nested_stacks(
    native: &[u8],
    target: V,
    ids: &ComposedMappings,
    flagged: bool,
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?;
    if !(0..=4096).contains(&count.0) {
        return Err(ReadingError::Message(
            "nested item count out of bounds".into(),
        ));
    }
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&count).r()?;
    for _ in 0..count.0 {
        if flagged {
            let present = cursor.get_bool()?;
            out.write_bool(present).r()?;
            if !present {
                continue;
            }
        }
        let template = TEMPLATE_ITEM.read(&mut cursor)?;
        let rewritten = super::item::StructuredItemRewriter::to_version(&template, target, ids);
        TEMPLATE_ITEM.write(&mut out, &rewritten).r()?;
    }
    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in nested item list: {}",
            cursor.len()
        )));
    }
    Ok(out)
}

fn nested_stack_array(
    native: &[u8],
    target: V,
    ids: &ComposedMappings,
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?;
    if !(0..=4096).contains(&count.0) {
        return Err(ReadingError::Message(
            "nested item count out of bounds".into(),
        ));
    }
    let mut out = Vec::with_capacity(native.len());
    out.write_var_int(&count).r()?;
    for _ in 0..count.0 {
        let template = TEMPLATE_ITEM.read(&mut cursor)?;
        let rewritten = super::item::StructuredItemRewriter::to_version(&template, target, ids);
        TEMPLATE_ITEM.write(&mut out, &rewritten).r()?;
    }
    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in nested item list: {}",
            cursor.len()
        )));
    }
    Ok(out)
}

fn nested_stack(native: &[u8], target: V, ids: &ComposedMappings) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let template = TEMPLATE_ITEM.read(&mut cursor)?;
    if !cursor.is_empty() {
        return Err(ReadingError::Message(format!(
            "trailing bytes in nested item: {}",
            cursor.len()
        )));
    }
    let rewritten = super::item::StructuredItemRewriter::to_version(&template, target, ids);
    let mut out = Vec::new();
    TEMPLATE_ITEM.write(&mut out, &rewritten).r()?;
    Ok(out)
}

fn map(mapping: &crate::api::IdMapping, id: i32) -> Option<i32> {
    let id = u32::try_from(id).ok()?;
    i32::try_from(mapping.map(id)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::VAR_INT;
    use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
    use pumpkin_protocol::ser::NetworkWriteExt;

    fn ids() -> &'static ComposedMappings {
        crate::api::MappingData::get().composed(V::V_26_3)
    }

    #[test]
    fn map_decoration_types_downgrade_without_changing_other_entry_data() {
        const NEW_TYPES: [&str; 5] = [
            "abandoned_camp",
            "ancient_city",
            "desert_pyramid",
            "mineshaft",
            "ocean_ruin_warm",
        ];
        let mut decorations = NbtCompound::new();
        for (index, decoration_type) in NEW_TYPES.into_iter().enumerate() {
            let mut entry = NbtCompound::new();
            entry.put_string("type", decoration_type.to_owned());
            entry.put_int("x", index as i32 - 2);
            entry.put_int("z", 17 + index as i32);
            entry.put_float("rotation", 1.25);
            decorations.put(&format!("new_{index}"), NbtTag::Compound(entry));
        }
        let mut unchanged = NbtCompound::new();
        unchanged.put_string("type", "village_desert".to_owned());
        unchanged.put_int("x", 100);
        decorations.put("existing", NbtTag::Compound(unchanged));

        let native = NbtTag::Compound(decorations);
        let mut payload = Vec::new();
        payload
            .write_nbt_with_version(Some(&native), &V::V_26_3)
            .unwrap();
        let downgraded = to_version(DataComponent::MapDecorations, &payload, V::V_26_2, ids())
            .expect("26.2 has a map-decoration component");
        let mut reader = downgraded.as_slice();
        let Some(NbtTag::Compound(downgraded)) = reader.get_nbt(&V::V_26_3).unwrap() else {
            panic!("downgraded map decorations remain a compound");
        };
        assert!(reader.is_empty());

        for index in 0..NEW_TYPES.len() {
            let entry = downgraded
                .get(&format!("new_{index}"))
                .and_then(NbtTag::extract_compound)
                .expect("decoration entry");
            assert_eq!(entry.get_string("type").as_deref(), Some("village_plains"));
            assert_eq!(entry.get_int("x"), Some(index as i32 - 2));
            assert_eq!(entry.get_int("z"), Some(17 + index as i32));
            assert_eq!(entry.get_float("rotation"), Some(1.25));
        }
        let entry = downgraded
            .get("existing")
            .and_then(NbtTag::extract_compound)
            .expect("existing decoration entry");
        assert_eq!(entry.get_string("type").as_deref(), Some("village_desert"));
        assert_eq!(entry.get_int("x"), Some(100));
    }

    /// `md('1.21.4')` ends `enchantments` with `showTooltip`; `md('1.21.5')`
    /// does not.
    #[test]
    fn enchantments_gain_a_tooltip_flag_below_1_21_5() {
        let native = vec![1, 33, 5];
        assert_eq!(
            to_version(DataComponent::Enchantments, &native, V::V_1_21_4, ids()),
            Some(vec![1, 33, 5, 1])
        );
        assert_eq!(
            to_version(DataComponent::Enchantments, &native, V::V_1_21_5, ids()),
            Some(native)
        );
    }

    #[test]
    fn adventure_predicates_keep_block_sets_and_tooltip_visibility() {
        let target = V::V_1_21_4;
        let mappings = crate::api::MappingData::get().composed(target);
        let source_block = registry_value_id(V::V_26_3, "block", "stone");
        let mapped_block = mappings.blocks.map(source_block as u32).unwrap() as i32;
        let mut native = Vec::new();
        VAR_INT.write(&mut native, &VarInt(1)).unwrap(); // predicates
        native.push(1); // block holder set is present
        VAR_INT.write(&mut native, &VarInt(2)).unwrap(); // one explicit block id
        VAR_INT.write(&mut native, &VarInt(source_block)).unwrap();
        native.extend([0, 0]); // no property or NBT predicates
        VAR_INT.write(&mut native, &VarInt(0)).unwrap(); // no component matchers
        VAR_INT.write(&mut native, &VarInt(0)).unwrap(); // no extra requirements

        for component in [DataComponent::CanPlaceOn, DataComponent::CanBreak] {
            for show_in_tooltip in [true, false] {
                let encoded =
                    to_version_with_tooltip(component, &native, target, mappings, show_in_tooltip)
                        .expect("1.21.4 retains the adventure block predicate");
                let mut cursor = encoded.as_slice();
                assert_eq!(cursor.get_var_int().unwrap().0, 1);
                assert!(cursor.get_bool().unwrap());
                assert_eq!(cursor.get_var_int().unwrap().0, 2);
                assert_eq!(cursor.get_var_int().unwrap().0, mapped_block);
                assert!(!cursor.get_bool().unwrap());
                assert!(!cursor.get_bool().unwrap());
                assert_eq!(cursor.get_bool().unwrap(), show_in_tooltip);
                assert!(cursor.is_empty());
            }
        }
    }

    /// `md('1.21.4')` types `unbreakable` as `bool`, `md('1.21.5')` as `void`.
    #[test]
    fn unbreakable_is_a_bool_below_1_21_5() {
        assert_eq!(
            to_version(DataComponent::Unbreakable, &[], V::V_1_21_4, ids()),
            Some(vec![1])
        );
    }

    /// `md('1.21.9')` prefixes `entity_data` with the entity type;
    /// `md('1.21.8')` starts at the NBT.
    #[test]
    fn entity_data_loses_its_type_prefix_below_1_21_9() {
        let native = vec![7, 0];
        assert_eq!(
            to_version(DataComponent::EntityData, &native, V::V_1_21_7, ids()),
            Some(vec![0])
        );
    }

    /// Components without a safe converter remain fail-closed.
    #[test]
    fn a_component_without_a_converter_is_dropped() {
        assert_eq!(
            to_version(
                DataComponent::IntangibleProjectile,
                &[0],
                V::V_1_21_4,
                ids()
            ),
            None
        );
    }

    #[test]
    fn jukebox_song_ids_map_by_name_and_inline_songs_are_consumed() {
        let target = V::V_1_21_4;
        let source_id = registry_value_id(V::V_26_3, "jukebox_song", "cat");
        let target_id = registry_value_id(target, "jukebox_song", "cat");
        let mut native = Vec::new();
        VAR_INT.write(&mut native, &VarInt(source_id)).unwrap();
        let mut expected = vec![1]; // the older holder arm
        VAR_INT
            .write(&mut expected, &VarInt(target_id + 1))
            .unwrap();
        expected.push(1); // visible in the legacy tooltip
        assert_eq!(
            to_version(
                DataComponent::JukeboxPlayable,
                &native,
                target,
                crate::api::MappingData::get().composed(target),
            ),
            Some(expected)
        );

        let mut holder = vec![1];
        VAR_INT.write(&mut holder, &VarInt(target_id + 1)).unwrap();
        holder.push(1); // 1.21.4 show_in_tooltip
        holder.push(0x7f); // next unlength-prefixed component
        let mut cursor = holder.as_slice();
        assert_eq!(
            legacy_jukebox_playable_to_native(&mut cursor, target, false).unwrap(),
            Some(native.clone())
        );
        assert_eq!(cursor, &[0x7f]);

        let mut inline = vec![1, 0, 0]; // holder arm; inline song; inline sound
        inline.write_string("minecraft:music_disc.cat").unwrap();
        inline.push(0); // no fixed range
        inline.push(0); // no song description NBT
        inline.extend(180.0_f32.to_bits().to_be_bytes());
        VAR_INT.write(&mut inline, &VarInt(15)).unwrap();
        inline.push(1); // 1.21.4 show_in_tooltip
        inline.push(0x7f);
        let mut cursor = inline.as_slice();
        assert_eq!(
            legacy_jukebox_playable_to_native(&mut cursor, target, false).unwrap(),
            None,
            "dynamic inline jukebox data cannot become a static registry id"
        );
        assert_eq!(cursor, &[0x7f]);
    }

    #[test]
    fn trim_instrument_and_material_holder_ids_map_by_registry_name() {
        let target = V::V_1_20_5;
        let ids = crate::api::MappingData::get().composed(target);
        let mut native = Vec::new();
        VAR_INT
            .write(
                &mut native,
                &VarInt(registry_value_id(V::V_26_3, "instrument", "ponder_goat_horn") + 1),
            )
            .unwrap();
        let encoded = to_version(DataComponent::Instrument, &native, target, ids).unwrap();
        let mut shared = encoded.clone();
        shared.extend([0x55, 0x66]); // Following component/header bytes.
        let mut cursor = shared.as_slice();
        assert_eq!(
            legacy_registry_component_to_native(
                DataComponent::Instrument,
                &mut cursor,
                target,
                false,
            )
            .unwrap(),
            Some(native.clone())
        );
        assert_eq!(cursor, &[0x55, 0x66]);
        let mut bounded = shared.as_slice();
        assert!(
            legacy_registry_component_to_native(
                DataComponent::Instrument,
                &mut bounded,
                target,
                true,
            )
            .is_err()
        );

        let target = V::V_1_21_5;
        let ids = crate::api::MappingData::get().composed(target);
        let mut native_trim = Vec::new();
        for (registry, name) in [("trim_material", "iron"), ("trim_pattern", "coast")] {
            VAR_INT
                .write(
                    &mut native_trim,
                    &VarInt(registry_value_id(V::V_26_3, registry, name) + 1),
                )
                .unwrap();
        }
        let mut expected_trim = Vec::new();
        for (registry, name) in [("trim_material", "iron"), ("trim_pattern", "coast")] {
            VAR_INT
                .write(
                    &mut expected_trim,
                    &VarInt(registry_value_id(target, registry, name) + 1),
                )
                .unwrap();
        }
        assert_eq!(
            to_version(DataComponent::Trim, &native_trim, target, ids),
            Some(expected_trim.clone())
        );
        assert_eq!(
            registry_ids_to_native(DataComponent::Trim, &expected_trim, target).unwrap(),
            native_trim
        );

        let instrument = registry_value_id(V::V_26_3, "instrument", "ponder_goat_horn");
        let mut native_instrument = Vec::new();
        VAR_INT
            .write(&mut native_instrument, &VarInt(instrument + 1))
            .unwrap();
        let mapped_instrument =
            to_version(DataComponent::Instrument, &native_instrument, target, ids).unwrap();
        let mut expected_instrument = Vec::new();
        expected_instrument.push(1); // holder arm in 1.21.5+
        VAR_INT
            .write(
                &mut expected_instrument,
                &VarInt(registry_value_id(target, "instrument", "ponder_goat_horn") + 1),
            )
            .unwrap();
        assert_eq!(mapped_instrument, expected_instrument);
        let mut instrument_payload = expected_instrument.as_slice();
        assert_eq!(
            legacy_registry_component_to_native(
                DataComponent::Instrument,
                &mut instrument_payload,
                target,
                true,
            )
            .unwrap(),
            Some(native_instrument)
        );

        let target = V::V_1_21_5;
        let ids = crate::api::MappingData::get().composed(target);
        let material = registry_value_id(V::V_26_3, "trim_material", "redstone");
        let mut native_material = Vec::new();
        VAR_INT
            .write(&mut native_material, &VarInt(material + 1))
            .unwrap();
        let mapped_material = to_version(
            DataComponent::ProvidesTrimMaterial,
            &native_material,
            target,
            ids,
        )
        .unwrap();
        let mut expected_material = Vec::new();
        expected_material.push(1); // holder arm in the 1.21.5 EitherHolder
        VAR_INT
            .write(
                &mut expected_material,
                &VarInt(registry_value_id(target, "trim_material", "redstone") + 1),
            )
            .unwrap();
        assert_eq!(mapped_material, expected_material);
        let mut material_payload = expected_material.as_slice();
        assert_eq!(
            legacy_registry_component_to_native(
                DataComponent::ProvidesTrimMaterial,
                &mut material_payload,
                target,
                true,
            )
            .unwrap(),
            Some(native_material)
        );
    }

    #[test]
    fn legacy_trim_and_instrument_holders_round_trip_for_1_21_4() {
        let target = V::V_1_21_4;
        let ids = crate::api::MappingData::get().composed(target);
        let mut native_trim = Vec::new();
        for (registry, name) in [("trim_material", "iron"), ("trim_pattern", "coast")] {
            VAR_INT
                .write(
                    &mut native_trim,
                    &VarInt(registry_value_id(V::V_26_3, registry, name) + 1),
                )
                .unwrap();
        }
        let legacy_trim = to_version(DataComponent::Trim, &native_trim, target, ids).unwrap();
        let mut read = legacy_trim.as_slice();
        for (registry, name) in [("trim_material", "iron"), ("trim_pattern", "coast")] {
            assert_eq!(
                read.get_var_int().unwrap().0,
                registry_value_id(target, registry, name) + 1
            );
        }
        assert!(read.get_bool().unwrap(), "trim remains visible in tooltips");
        assert!(read.is_empty());
        let mut trim_payload = legacy_trim.as_slice();
        assert_eq!(
            legacy_registry_component_to_native(
                DataComponent::Trim,
                &mut trim_payload,
                target,
                true,
            )
            .unwrap(),
            Some(native_trim)
        );

        let mut native_instrument = Vec::new();
        VAR_INT
            .write(
                &mut native_instrument,
                &VarInt(registry_value_id(V::V_26_3, "instrument", "ponder_goat_horn") + 1),
            )
            .unwrap();
        let legacy_instrument =
            to_version(DataComponent::Instrument, &native_instrument, target, ids).unwrap();
        let mut read = legacy_instrument.as_slice();
        assert_eq!(
            read.get_var_int().unwrap().0,
            registry_value_id(target, "instrument", "ponder_goat_horn") + 1
        );
        assert!(read.is_empty());
        let mut instrument_payload = legacy_instrument.as_slice();
        assert_eq!(
            legacy_registry_component_to_native(
                DataComponent::Instrument,
                &mut instrument_payload,
                target,
                true,
            )
            .unwrap(),
            Some(native_instrument)
        );

        let target = V::V_1_21_5;
        let mut native_instrument = Vec::new();
        VAR_INT
            .write(
                &mut native_instrument,
                &VarInt(registry_value_id(V::V_26_3, "instrument", "ponder_goat_horn") + 1),
            )
            .unwrap();
        let mut string_arm = vec![0]; // direct resource-location arm
        string_arm
            .write_string("minecraft:ponder_goat_horn")
            .unwrap();
        let mut string_payload = string_arm.as_slice();
        assert_eq!(
            legacy_registry_component_to_native(
                DataComponent::Instrument,
                &mut string_payload,
                target,
                true,
            )
            .unwrap(),
            Some(native_instrument)
        );

        let target = V::V_26_2;
        let ids = crate::api::MappingData::get().composed(target);
        let mut native_instrument = Vec::new();
        VAR_INT
            .write(
                &mut native_instrument,
                &VarInt(registry_value_id(V::V_26_3, "instrument", "ponder_goat_horn") + 1),
            )
            .unwrap();
        let encoded =
            to_version(DataComponent::Instrument, &native_instrument, target, ids).unwrap();
        let mut expected = Vec::new();
        VAR_INT
            .write(
                &mut expected,
                &VarInt(registry_value_id(target, "instrument", "ponder_goat_horn") + 1),
            )
            .unwrap();
        assert_eq!(
            encoded, expected,
            "26.2 uses a holder directly, without the 1.21.5 EitherHolder flag"
        );

        let mut native_trim = Vec::new();
        for (registry, name) in [("trim_material", "iron"), ("trim_pattern", "coast")] {
            VAR_INT
                .write(
                    &mut native_trim,
                    &VarInt(registry_value_id(V::V_26_3, registry, name) + 1),
                )
                .unwrap();
        }
        let encoded = to_version(DataComponent::Trim, &native_trim, target, ids).unwrap();
        let mut expected = Vec::new();
        for (registry, name) in [("trim_material", "iron"), ("trim_pattern", "coast")] {
            VAR_INT
                .write(
                    &mut expected,
                    &VarInt(registry_value_id(target, registry, name) + 1),
                )
                .unwrap();
        }
        assert_eq!(encoded, expected, "26.2 trim has no tooltip boolean");

        let native_material = {
            let mut bytes = Vec::new();
            VAR_INT
                .write(
                    &mut bytes,
                    &VarInt(registry_value_id(V::V_26_3, "trim_material", "redstone") + 1),
                )
                .unwrap();
            bytes
        };
        let encoded = to_version(
            DataComponent::ProvidesTrimMaterial,
            &native_material,
            target,
            ids,
        )
        .unwrap();
        let mut expected = Vec::new();
        VAR_INT
            .write(
                &mut expected,
                &VarInt(registry_value_id(target, "trim_material", "redstone") + 1),
            )
            .unwrap();
        assert_eq!(
            encoded, expected,
            "26.2 uses a holder directly for provides_trim_material"
        );
    }

    #[test]
    fn custom_model_data_downgrades_only_a_single_whole_float_before_1_21_4() {
        let mut native = vec![1];
        native.extend(7.0f32.to_bits().to_be_bytes());
        native.extend([0, 0, 0]);
        let expected = {
            let mut value = Vec::new();
            VAR_INT.write(&mut value, &VarInt(7)).unwrap();
            value
        };
        assert_eq!(
            to_version(DataComponent::CustomModelData, &native, V::V_1_21_2, ids()),
            Some(expected)
        );
        assert_eq!(
            to_version(DataComponent::CustomModelData, &native, V::V_1_21_5, ids()),
            Some(native.clone())
        );
        assert_eq!(
            to_version(DataComponent::CustomModelData, &native, V::V_1_21_4, ids()),
            Some(native.clone())
        );

        let mut fractional = vec![1];
        fractional.extend(7.5f32.to_bits().to_be_bytes());
        fractional.extend([0, 0, 0]);
        assert_eq!(
            to_version(
                DataComponent::CustomModelData,
                &fractional,
                V::V_1_21_2,
                ids()
            ),
            None
        );
    }

    #[test]
    fn an_unknown_source_registry_id_fails_closed() {
        let target = V::V_26_2;
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(127)).unwrap();
        VAR_INT
            .write(
                &mut payload,
                &VarInt(registry_value_id(V::V_26_3, "trim_pattern", "coast")),
            )
            .unwrap();
        assert_eq!(
            to_version(
                DataComponent::Trim,
                &payload,
                target,
                crate::api::MappingData::get().composed(target),
            ),
            None
        );
    }

    fn registry_value_id(version: V, registry_id: &str, name: &str) -> i32 {
        let index = if version >= V::V_26_3 {
            pumpkin_data::registry::REGISTRY_V_26_3
                .iter()
                .find(|registry| registry.registry_id == registry_id)
                .unwrap()
                .entries
                .iter()
                .position(|entry| entry.name == name)
        } else {
            crate::registry::generated::get_synced(version)
                .unwrap()
                .iter()
                .find(|registry| registry.registry_id == registry_id)
                .unwrap()
                .entries
                .iter()
                .position(|entry| entry.name == name)
        }
        .unwrap();
        i32::try_from(index).unwrap()
    }

    #[test]
    fn negative_id_set_lengths_are_rejected() {
        for count in [-1, i32::MIN] {
            let mut payload = Vec::new();
            VAR_INT.write(&mut payload, &VarInt(count)).unwrap();
            let mut cursor = payload.as_slice();
            let mut output = Vec::new();
            assert!(copy_id_set(&mut cursor, &mut output).is_err());
        }
    }

    #[test]
    fn unmappable_painting_holder_is_rejected_instead_of_becoming_inline() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(-1)).unwrap();
        assert!(holder_id(&payload, &ids().paintings).is_err());
    }

    #[test]
    fn nested_item_lists_reject_negative_counts() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(-1)).unwrap();
        assert!(nested_stacks(&payload, V::V_1_21_5, ids(), true).is_err());
        assert!(nested_stack_array(&payload, V::V_1_21_5, ids()).is_err());
    }

    fn consume_effect_component_payload(
        component: DataComponent,
        directional_particles: Option<bool>,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        if component == DataComponent::Consumable {
            payload.write_f32_be(1.25).unwrap();
            payload.write_var_int(&VarInt(0)).unwrap(); // animation
            payload.write_var_int(&VarInt(1)).unwrap(); // sound holder
            payload.write_bool(false).unwrap(); // consume particles
        }
        payload.write_var_int(&VarInt(1)).unwrap(); // effect count
        payload.write_var_int(&VarInt(3)).unwrap(); // teleport randomly
        payload.write_f32_be(16.0).unwrap();
        if let Some(directional_particles) = directional_particles {
            payload.write_bool(directional_particles).unwrap();
        }
        payload
    }

    #[test]
    fn the_26_3_consume_effect_flag_is_removed_and_defaulted_for_26_2() {
        for component in [DataComponent::Consumable, DataComponent::DeathProtection] {
            let native = consume_effect_component_payload(component, Some(false));
            let client = to_version(component, &native, V::V_26_2, ids()).unwrap();
            assert_eq!(
                client,
                consume_effect_component_payload(component, None),
                "{} drops the 26.3-only field",
                component.to_name()
            );
            assert_eq!(
                consume_effects_to_native(component, &client, V::V_26_2).unwrap(),
                consume_effect_component_payload(component, Some(true)),
                "older clients default to directional particles"
            );
        }
    }
}

/// Expands a client-side pre-26.3 consume effect to the 26.3 form. The
/// 26.2 format has no `directional_particles` field and defaults it to true.
pub fn consume_effects_to_native(
    component: DataComponent,
    client: &[u8],
    source: V,
) -> Result<Vec<u8>, ReadingError> {
    if source >= V::V_26_3 {
        return Ok(client.to_vec());
    }
    consume_effect_payload(component, client, false, true)
}

fn consume_effect_payload(
    component: DataComponent,
    native: &[u8],
    input_has_directional_particles: bool,
    output_has_directional_particles: bool,
) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let mut out = Vec::with_capacity(native.len().saturating_add(4));
    if component == DataComponent::Consumable {
        for shape in [Shape::F32, Shape::VarInt, Shape::Sound, Shape::Bool] {
            copy_shape(&shape, &mut cursor, &mut out)?;
        }
    } else if component != DataComponent::DeathProtection {
        return Err(ReadingError::Message(
            "consume-effect converter used for an unrelated component".into(),
        ));
    }

    let count = cursor.get_var_int()?.0;
    if !(0..=4096).contains(&count) {
        return Err(ReadingError::Message(
            "consume-effect count out of bounds".into(),
        ));
    }
    out.write_var_int(&VarInt(count)).r()?;
    for _ in 0..count {
        copy_consume_effect(
            &mut cursor,
            &mut out,
            input_has_directional_particles,
            output_has_directional_particles,
        )?;
    }
    if !cursor.is_empty() {
        return Err(ReadingError::Message(
            "trailing bytes in consume-effect component".into(),
        ));
    }
    Ok(out)
}

fn copy_consume_effect(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    input_has_directional_particles: bool,
    output_has_directional_particles: bool,
) -> Result<(), ReadingError> {
    let effect_type = cursor.get_var_int()?.0;
    out.write_var_int(&VarInt(effect_type)).r()?;
    match effect_type {
        0 => {
            copy_shape(&Shape::StatusEffects, cursor, out)?;
            copy_shape(&Shape::F32, cursor, out)?;
        }
        1 => copy_shape(&Shape::IdSet, cursor, out)?,
        2 => {}
        3 => {
            copy_shape(&Shape::F32, cursor, out)?;
            if input_has_directional_particles {
                let directional = cursor.get_bool()?;
                if output_has_directional_particles {
                    out.write_bool(directional).r()?;
                }
            } else if output_has_directional_particles {
                out.write_bool(true).r()?;
            }
        }
        4 => copy_shape(&Shape::Sound, cursor, out)?,
        other => {
            return Err(ReadingError::Message(format!(
                "unknown consume effect {other}"
            )));
        }
    }
    Ok(())
}

/// Combines 26.3 food data with its consumable effects and use remainder for
/// the pre-1.21.2 FoodProperties component.
pub fn food_to_legacy(
    food: &[u8],
    consumable: Option<&[u8]>,
    remainder: Option<&[u8]>,
    target: V,
    ids: &ComposedMappings,
) -> Result<Vec<u8>, ReadingError> {
    let mut food_cursor = food;
    let nutrition = food_cursor.get_var_int()?;
    let saturation = food_cursor.get_f32_be()?;
    let can_always_eat = food_cursor.get_bool()?;
    if !food_cursor.is_empty() {
        return Err(ReadingError::Message(
            "trailing bytes in food component".into(),
        ));
    }

    let mut eat_seconds = 1.6;
    let mut effects = Vec::new();
    if let Some(consumable) = consumable {
        let mut cursor = consumable;
        eat_seconds = cursor.get_f32_be()?;
        cursor.get_var_int()?; // Animation.
        item_shape::skip(&SOUND, &mut cursor)?;
        cursor.get_bool()?; // Consume particles.
        let count = cursor.get_var_int()?.0;
        if !(0..=4096).contains(&count) {
            return Err(ReadingError::Message(
                "consume effect count out of bounds".into(),
            ));
        }
        for _ in 0..count {
            let effect_type = cursor.get_var_int()?.0;
            if effect_type != 0 {
                // Via only maps apply_status_effects into the old food list.
                let mut encoded = Vec::new();
                encoded.write_var_int(&VarInt(effect_type)).r()?;
                copy_consume_effect_tail(&mut cursor, &mut encoded, effect_type, true, false)?;
                continue;
            }

            let status_count = cursor.get_var_int()?.0;
            if !(0..=4096).contains(&status_count) {
                return Err(ReadingError::Message(
                    "food effect count out of bounds".into(),
                ));
            }
            let mut statuses = Vec::with_capacity(status_count as usize);
            for _ in 0..status_count {
                let source_id = cursor.get_var_int()?.0;
                let data_start = cursor;
                item_shape::skip_effect_parameters(&mut cursor)?;
                let data = data_start[..data_start.len() - cursor.len()].to_vec();
                let target_id = registry_entry_name(V::V_26_3, "mob_effect", source_id)
                    .and_then(|name| registry_entry_id(target, "mob_effect", name));
                if let Some(target_id) = target_id {
                    statuses.push((target_id, data));
                }
            }
            let probability = cursor.get_f32_be()?;
            for (id, data) in statuses {
                effects.push((id, data, probability));
            }
        }
        if !cursor.is_empty() {
            return Err(ReadingError::Message(
                "trailing bytes in consumable component".into(),
            ));
        }
    }

    let mut out = Vec::new();
    out.write_var_int(&nutrition).r()?;
    out.write_f32_be(saturation).r()?;
    out.write_bool(can_always_eat).r()?;
    if target >= V::V_1_21 {
        out.write_f32_be(eat_seconds).r()?;
        if let Some(remainder) = remainder {
            let mut cursor = remainder;
            let item = TEMPLATE_ITEM.read(&mut cursor)?;
            if !cursor.is_empty() {
                return Err(ReadingError::Message(
                    "trailing bytes in food remainder".into(),
                ));
            }
            let mapped = super::item::StructuredItemRewriter::to_version(&item, target, ids);
            ItemT::for_version(target).write(&mut out, &mapped).r()?;
        } else {
            ItemT::for_version(target)
                .write(&mut out, &crate::api::types::Item::Empty)
                .r()?;
        }
    }
    out.write_var_int(&VarInt(i32::try_from(effects.len()).map_err(|_| {
        ReadingError::Message("food effect count overflow".into())
    })?))
    .r()?;
    for (id, data, probability) in effects {
        out.write_var_int(&VarInt(id)).r()?;
        out.write_slice(&data).r()?;
        out.write_f32_be(probability).r()?;
    }
    Ok(out)
}

fn copy_consume_effect_tail(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    effect_type: i32,
    input_has_directional_particles: bool,
    output_has_directional_particles: bool,
) -> Result<(), ReadingError> {
    match effect_type {
        0 => {
            copy_shape(&Shape::StatusEffects, cursor, out)?;
            copy_shape(&Shape::F32, cursor, out)?;
        }
        1 => copy_shape(&Shape::IdSet, cursor, out)?,
        2 => {}
        3 => {
            copy_shape(&Shape::F32, cursor, out)?;
            if input_has_directional_particles {
                let value = cursor.get_bool()?;
                if output_has_directional_particles {
                    out.write_bool(value).r()?;
                }
            } else if output_has_directional_particles {
                out.write_bool(true).r()?;
            }
        }
        4 => copy_shape(&Shape::Sound, cursor, out)?,
        other => {
            return Err(ReadingError::Message(format!(
                "unknown consume effect {other}"
            )));
        }
    }
    Ok(())
}

fn copy_shape(shape: &Shape, cursor: &mut &[u8], out: &mut Vec<u8>) -> Result<(), ReadingError> {
    let source = *cursor;
    item_shape::skip(shape, cursor)?;
    let consumed = source.len() - cursor.len();
    out.write_slice(&source[..consumed]).r()
}
