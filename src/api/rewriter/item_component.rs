use pumpkin_data::data_component::DataComponent;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt, ReadingError};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::ComposedMappings;
use crate::api::rewriter::item_shape::{self, Shape};
use crate::api::types::{TEMPLATE_ITEM, WireType};

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
        | C::IntangibleProjectile
        | C::Trim
        | C::Instrument => V::V_1_21_5,
        C::CustomModelData | C::Food => V::V_1_21_2,
        _ => V::V_1_20_5,
    }
}

/// Core writes these as an inline holder and then no inline data, which no
/// version can decode.
const fn is_empty_holder(component: DataComponent) -> bool {
    matches!(
        component,
        DataComponent::Trim | DataComponent::Instrument | DataComponent::ProvidesTrimMaterial
    )
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
    let mapped = map_nested_ids(component, native, target, ids).ok()?;
    if target >= V::V_26_3 {
        return Some(mapped);
    }
    if is_empty_holder(component) {
        return None;
    }
    if target >= shape_floor(component) {
        return Some(mapped);
    }
    adapt(component, mapped, target).ok().flatten()
}

fn adapt(
    component: DataComponent,
    native: Vec<u8>,
    target: V,
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
            let mut out = Vec::with_capacity(native.len() + 2);
            out.push(1);
            out.extend_from_slice(&native);
            if target <= V::V_1_21_4 {
                out.push(1);
            }
            out
        }
        C::AttributeModifiers => attribute_modifiers(&native, target)?,
        C::Equippable => match equippable(&native, target)? {
            Some(out) => out,
            None => return Ok(None),
        },
        C::EntityData | C::BlockEntityData => {
            let mut cursor = native.as_slice();
            cursor.get_var_int()?;
            cursor.to_vec()
        }
        C::Profile => profile(&native)?,
        // can_place_on, can_break, intangible_projectile, custom_model_data,
        // food and anything else whose shape moved has no converter.
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
        C::Enchantments | C::StoredEnchantments => enchantment_ids(native, ids),
        C::AttributeModifiers => attribute_ids(native, ids),
        C::Consumable | C::DeathProtection if target < V::V_26_3 => {
            consume_effect_payload(component, native, true, false)
        }
        C::EntityData => leading_id(native, &ids.entities),
        C::BlockEntityData => leading_id(native, &ids.blockentities),
        C::PaintingVariant => holder_id(native, &ids.paintings),
        C::BundleContents => nested_stacks(native, target, ids, false),
        C::Container => nested_stacks(native, target, ids, true),
        _ => Ok(native.to_vec()),
    }
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
    let mapped = map(mapping, raw - 1).map_or(0, |id| id + 1);
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

    fn ids() -> &'static ComposedMappings {
        crate::api::MappingData::get().composed(V::V_26_3)
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

    /// `md('1.21.4')` has no `can_place_on` that matches 26.3's, and nothing
    /// converts it, so it must not be sent as it is.
    #[test]
    fn a_component_without_a_converter_is_dropped() {
        assert_eq!(
            to_version(DataComponent::CanPlaceOn, &[0, 1], V::V_1_21_4, ids()),
            None
        );
    }

    /// Core writes both as an inline holder with no inline data.
    #[test]
    fn trim_and_instrument_never_reach_an_older_client() {
        assert_eq!(
            to_version(DataComponent::Trim, &[0, 0], V::V_1_21_5, ids()),
            None
        );
        assert_eq!(
            to_version(DataComponent::Instrument, &[0], V::V_26_2, ids()),
            None
        );
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
                "{component:?} drops the 26.3-only field"
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

fn copy_shape(shape: &Shape, cursor: &mut &[u8], out: &mut Vec<u8>) -> Result<(), ReadingError> {
    let source = *cursor;
    item_shape::skip(shape, cursor)?;
    let consumed = source.len() - cursor.len();
    out.write_slice(&source[..consumed]).r()
}
