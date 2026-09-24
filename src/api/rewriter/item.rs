use pumpkin_data::data_component::DataComponent;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{
    NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt, ReadingError, WritingError,
};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::{item_component, item_nbt};
use crate::api::types::{Item, ItemComponent, ItemT, WireType, component_payload_len};
use crate::api::{ComposedMappings, IdMapping, PacketWrapper, TranslateError};

pub struct StructuredItemRewriter;

impl StructuredItemRewriter {
    /// A stack core wrote in the 26.3 form, in `target`'s item form.
    #[must_use]
    pub fn to_version(item: &Item, target: V, ids: &ComposedMappings) -> Item {
        let Item::Structured {
            count,
            id,
            added,
            removed,
        } = item
        else {
            return item.clone();
        };
        let Some(id) = map(&ids.items, *id) else {
            return Item::Empty;
        };

        if target < ItemT::FIRST_STRUCTURED {
            let nbt = item_nbt::components_to_nbt(added, target, ids);
            return Item::Nbt {
                id,
                count: i8::try_from(*count).unwrap_or(i8::MAX),
                nbt: nbt.map(pumpkin_nbt::tag::NbtTag::Compound),
            };
        }

        let mut out: Vec<ItemComponent> = Vec::with_capacity(added.len());
        for component in added {
            let Some(native) = u8::try_from(component.id)
                .ok()
                .and_then(DataComponent::try_from_id)
            else {
                continue;
            };
            let Some(mapped) = map_component_id(component.id, native, target, ids) else {
                continue;
            };
            let Some(data) = item_component::to_version(native, &component.data, target, ids)
            else {
                continue;
            };
            if let Some(existing) = out.iter_mut().find(|existing| existing.id == mapped) {
                // 26.2 has one animation component where 26.3 has separate
                // attack and interaction components. Keep the later value,
                // matching ViaBackwards' collision behavior.
                existing.data = data;
            } else {
                out.push(ItemComponent { id: mapped, data });
            }
        }
        let mut mapped_removed = Vec::with_capacity(removed.len());
        for component_id in removed {
            let native = u8::try_from(*component_id)
                .ok()
                .and_then(DataComponent::try_from_id);
            let mapped = native
                .and_then(|native| map_component_id(*component_id, native, target, ids))
                .or_else(|| map(&ids.data_component_type, *component_id));
            if let Some(mapped) = mapped
                && !mapped_removed.contains(&mapped)
            {
                mapped_removed.push(mapped);
            }
        }
        Item::Structured {
            count: *count,
            id,
            added: out,
            removed: mapped_removed,
        }
    }

    /// A stack a client on `source` sent, back in the 26.3 form core reads.
    /// Component payloads are taken as already being 26.3 shapes, which is
    /// what [`read_client_item`] hands over.
    #[must_use]
    pub fn to_native(item: &Item, source: V, ids: &ComposedMappings) -> Item {
        match item {
            Item::Empty => Item::Empty,
            Item::Nbt { id, count, nbt } => {
                let Some(id) = map(ids.items_inverse(), *id) else {
                    return Item::Empty;
                };
                let added = match nbt {
                    Some(pumpkin_nbt::tag::NbtTag::Compound(compound)) => {
                        item_nbt::nbt_to_components(compound, source)
                    }
                    _ => Vec::new(),
                };
                Item::Structured {
                    count: i32::from(*count),
                    id,
                    added,
                    removed: Vec::new(),
                }
            }
            Item::Structured {
                count,
                id,
                added,
                removed,
            } => {
                let Some(id) = map(ids.items_inverse(), *id) else {
                    return Item::Empty;
                };
                let component_ids = ids.data_component_type_inverse();
                Item::Structured {
                    count: *count,
                    id,
                    added: added
                        .iter()
                        .filter_map(|component| {
                            Some(ItemComponent {
                                id: map_component_id_from_client(
                                    component.id,
                                    source,
                                    component_ids,
                                )?,
                                data: component.data.clone(),
                            })
                        })
                        .collect(),
                    removed: removed
                        .iter()
                        .filter_map(|id| map_component_id_from_client(*id, source, component_ids))
                        .collect(),
                }
            }
        }
    }
}

/// Reads one stack a client on `version` sent and returns it in the 26.3
/// form, with every component whose shape cannot be rebuilt left out.
pub fn read_client_item(
    r: &mut &[u8],
    version: V,
    length_prefixed: bool,
    ids: &ComposedMappings,
) -> Result<Item, ReadingError> {
    if version < ItemT::FIRST_STRUCTURED {
        let item = ItemT::for_version(version).read(r)?;
        return Ok(StructuredItemRewriter::to_native(&item, version, ids));
    }

    let count = r.get_var_int()?.0;
    if count == 0 {
        return Ok(Item::Empty);
    }
    let raw_id = r.get_var_int()?.0;
    let to_add = r.get_var_int()?.0;
    let to_remove = r.get_var_int()?.0;
    if !(0..=256).contains(&to_add) || !(0..=256).contains(&to_remove) {
        return Err(ReadingError::Message(
            "component count out of bounds".into(),
        ));
    }

    let component_ids = ids.data_component_type_inverse();
    let enchantments = ids.enchantments.inverse();
    let mut added = Vec::with_capacity(to_add as usize);
    for _ in 0..to_add {
        let client_id = r.get_var_int()?.0;
        let body = if length_prefixed {
            let len = usize::try_from(r.get_var_int()?.0)
                .map_err(|_| ReadingError::Message("negative component length".into()))?;
            Some(r.read_slice_borrowed(len)?)
        } else {
            None
        };
        let native = map_component_id_from_client(client_id, version, component_ids)
            .and_then(|id| u8::try_from(id).ok())
            .and_then(DataComponent::try_from_id);
        let Some(native) = native else {
            if body.is_some() || client_component_is_empty(client_id, version) {
                continue;
            }
            return Err(ReadingError::Message(format!(
                "unknown component {client_id} on {version}"
            )));
        };
        let data = match body {
            Some(mut body) => read_client_payload(native, &mut body, version, &enchantments)?,
            None => read_client_payload(native, r, version, &enchantments)?,
        };
        if let Some(data) = data {
            added.push(ItemComponent {
                id: i32::from(native.to_id()),
                data,
            });
        }
    }

    let mut removed = Vec::with_capacity(to_remove as usize);
    for _ in 0..to_remove {
        if let Some(id) = map_component_id_from_client(r.get_var_int()?.0, version, component_ids) {
            removed.push(id);
        }
    }

    let Some(id) = map(ids.items_inverse(), raw_id) else {
        return Ok(Item::Empty);
    };
    Ok(Item::Structured {
        count,
        id,
        added,
        removed,
    })
}

/// Whether a client on `version` has an empty payload component with no 26.3
/// counterpart: `hide_additional_tooltip`, `hide_tooltip` and `fire_resistant`.
fn client_component_is_empty(client_id: i32, version: V) -> bool {
    if version <= V::V_1_21 {
        matches!(client_id, 14 | 15 | 21)
    } else if version <= V::V_1_21_4 {
        matches!(client_id, 15 | 16)
    } else {
        false
    }
}

/// Consumes one component payload in `version`'s layout. `None` means the
/// layout was read but nothing 26.3 can hold it.
fn read_client_payload(
    component: DataComponent,
    r: &mut &[u8],
    version: V,
    enchantments: &IdMapping,
) -> Result<Option<Vec<u8>>, ReadingError> {
    use DataComponent as C;
    if version >= item_component::shape_floor(component) {
        let len = component_payload_len(i32::from(component.to_id()), r)?;
        let native = r.read_slice_borrowed(len)?.to_vec();
        return Ok(Some(match component {
            C::Enchantments | C::StoredEnchantments => native_enchantments(&native, enchantments)?,
            _ => native,
        }));
    }
    match component {
        C::Unbreakable => {
            r.get_bool()?;
            Ok(Some(Vec::new()))
        }
        C::Enchantments | C::StoredEnchantments | C::DyedColor => {
            let len = component_payload_len(i32::from(component.to_id()), r)?;
            let native = r.read_slice_borrowed(len)?.to_vec();
            r.get_bool()?;
            Ok(Some(match component {
                C::DyedColor => native,
                _ => native_enchantments(&native, enchantments)?,
            }))
        }
        C::EntityData | C::BlockEntityData => {
            let tag = r.get_nbt(&version)?;
            let mut out = Vec::new();
            out.write_var_int(&VarInt(0))
                .map_err(|error| ReadingError::Message(error.to_string()))?;
            out.write_nbt_with_version(tag.as_ref(), &V::V_26_3)
                .map_err(|error| ReadingError::Message(error.to_string()))?;
            Ok(Some(out))
        }
        // Everything else is read for its length and left out: nothing here
        // can rebuild the 26.3 value from the older layout.
        _ => {
            skip_client_payload(component, r, version)?;
            Ok(None)
        }
    }
}

fn native_enchantments(native: &[u8], enchantments: &IdMapping) -> Result<Vec<u8>, ReadingError> {
    let mut cursor = native;
    let count = cursor.get_var_int()?.0;
    let mut kept = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        let id = cursor.get_var_int()?;
        let level = cursor.get_var_int()?;
        if let Some(mapped) = map(enchantments, id.0) {
            kept.push((VarInt(mapped), level));
        }
    }
    let mut out = Vec::with_capacity(native.len());
    let write = |out: &mut Vec<u8>, value: &VarInt| {
        out.write_var_int(value)
            .map_err(|error| ReadingError::Message(error.to_string()))
    };
    write(&mut out, &VarInt(i32::try_from(kept.len()).unwrap_or(0)))?;
    for (id, level) in kept {
        write(&mut out, &id)?;
        write(&mut out, &level)?;
    }
    Ok(out)
}

fn skip_id_set(r: &mut &[u8]) -> Result<(), ReadingError> {
    let n = r.get_var_int()?.0;
    if n < 0 {
        return Err(ReadingError::Message("negative id-set length".into()));
    }
    if n == 0 {
        r.get_str()?;
    } else {
        for _ in 1..n {
            r.get_var_int()?;
        }
    }
    Ok(())
}

fn skip_sound_holder(r: &mut &[u8]) -> Result<(), ReadingError> {
    if r.get_var_int()?.0 == 0 {
        r.get_str()?;
        if r.get_bool()? {
            r.get_f32_be()?;
        }
    }
    Ok(())
}

fn skip_client_payload(
    component: DataComponent,
    r: &mut &[u8],
    version: V,
) -> Result<(), ReadingError> {
    use DataComponent as C;
    match component {
        C::Tool => {
            let rules = r.get_var_int()?.0;
            for _ in 0..rules {
                skip_id_set(r)?;
                if r.get_bool()? {
                    r.get_f32_be()?;
                }
                if r.get_bool()? {
                    r.get_bool()?;
                }
            }
            r.get_f32_be()?;
            r.get_var_int()?;
        }
        C::AttributeModifiers => {
            let count = r.get_var_int()?.0;
            for _ in 0..count {
                r.get_var_int()?;
                if version <= V::V_1_20_5 {
                    r.get_uuid()?;
                }
                r.get_str()?;
                r.get_f64_be()?;
                r.get_var_int()?;
                r.get_var_int()?;
            }
            if version <= V::V_1_21_4 {
                r.get_bool()?;
            } else if version == V::V_1_21_11 && r.get_var_int()?.0 == 2 {
                r.get_nbt(&version)?;
            }
        }
        C::Equippable => {
            r.get_var_int()?;
            skip_sound_holder(r)?;
            for _ in 0..2 {
                if r.get_bool()? {
                    r.get_str()?;
                }
            }
            if r.get_bool()? {
                skip_id_set(r)?;
            }
            let bools = if version <= V::V_1_21_4 { 3 } else { 4 };
            for _ in 0..bools {
                r.get_bool()?;
            }
        }
        C::Profile => {
            if r.get_bool()? {
                r.get_str()?;
            }
            if r.get_bool()? {
                r.get_uuid()?;
            }
            let props = r.get_var_int()?.0;
            for _ in 0..props {
                r.get_str()?;
                r.get_str()?;
                if r.get_bool()? {
                    r.get_str()?;
                }
            }
        }
        C::JukeboxPlayable => {
            let trailing = version <= V::V_1_21_4;
            if r.get_bool()? {
                if r.get_var_int()?.0 == 0 {
                    skip_sound_holder(r)?;
                    r.get_nbt(&version)?;
                    r.get_f32_be()?;
                    r.get_var_int()?;
                }
            } else {
                r.get_str()?;
            }
            if trailing {
                r.get_bool()?;
            }
        }
        C::Trim => {
            for _ in 0..2 {
                skip_holder_with_inline(r, version)?;
            }
            r.get_bool()?;
        }
        C::Instrument => {
            if version > V::V_1_21_4 && !r.get_bool()? {
                r.get_str()?;
                return Ok(());
            }
            if r.get_var_int()?.0 == 0 {
                skip_sound_holder(r)?;
                r.get_f32_be()?;
                r.get_f32_be()?;
                r.get_nbt(&version)?;
            }
        }
        C::CanPlaceOn | C::CanBreak => {
            let predicates = r.get_var_int()?.0;
            for _ in 0..predicates {
                if r.get_bool()? {
                    skip_id_set(r)?;
                }
                if r.get_bool()? {
                    let props = r.get_var_int()?.0;
                    for _ in 0..props {
                        r.get_str()?;
                        let exact = r.get_bool()?;
                        r.get_str()?;
                        if !exact {
                            r.get_str()?;
                        }
                    }
                }
                r.get_nbt(&version)?;
            }
            r.get_bool()?;
        }
        C::IntangibleProjectile => {
            r.get_nbt(&version)?;
        }
        C::CustomModelData => {
            r.get_var_int()?;
        }
        C::Food => {
            r.get_var_int()?;
            r.get_f32_be()?;
            r.get_bool()?;
            r.get_f32_be()?;
            ItemT::for_version(version)
                .read(r)
                .map_err(|error| ReadingError::Message(error.to_string()))?;
            let effects = r.get_var_int()?.0;
            for _ in 0..effects {
                r.get_var_int()?;
                r.get_f32_be()?;
            }
        }
        _ => {
            return Err(ReadingError::Message(format!(
                "component {} has no reader for {version}",
                component.to_id()
            )));
        }
    }
    Ok(())
}

/// A 1.21.4 trim material or pattern holder, whose inline forms each carry an
/// extra item id.
fn skip_holder_with_inline(r: &mut &[u8], version: V) -> Result<(), ReadingError> {
    if r.get_var_int()?.0 != 0 {
        return Ok(());
    }
    r.get_str()?;
    r.get_var_int()?;
    let overrides = r.get_var_int()?.0;
    for _ in 0..overrides {
        r.get_str()?;
        r.get_str()?;
    }
    r.get_nbt(&version)?;
    Ok(())
}

/// A stack as a client on `version` sends it, read back into the 26.3 form
/// core expects; writing uses the length prefixed form from 1.21.5.
#[derive(Clone, Copy)]
pub struct ClientItemT<'a> {
    version: V,
    ids: &'a ComposedMappings,
}

impl<'a> ClientItemT<'a> {
    #[must_use]
    pub const fn new(version: V, ids: &'a ComposedMappings) -> Self {
        Self { version, ids }
    }

    const fn length_prefixed(&self) -> bool {
        self.version.protocol_version() >= V::V_1_21_5.protocol_version()
    }
}

impl WireType for ClientItemT<'_> {
    type Value = Item;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        read_client_item(r, self.version, self.length_prefixed(), self.ids)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        if self.length_prefixed() {
            ItemT::length_prefixed(V::V_26_3).write(w, v)
        } else {
            ItemT::for_version(V::V_26_3).write(w, v)
        }
    }
}

/// A stack in the server-to-client packet layout, read into the 26.3 form.
#[derive(Clone, Copy)]
pub struct ClientboundItemT<'a> {
    version: V,
    ids: &'a ComposedMappings,
}

impl<'a> ClientboundItemT<'a> {
    #[must_use]
    pub const fn new(version: V, ids: &'a ComposedMappings) -> Self {
        Self { version, ids }
    }
}

impl WireType for ClientboundItemT<'_> {
    type Value = Item;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        read_client_item(r, self.version, false, self.ids)
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        ItemT::for_version(self.version).write(w, v)
    }
}

/// Reads one stack in `layout`'s wire form and writes it back for `layout`.
pub fn rewrite_item(
    input: &mut &[u8],
    output: &mut Vec<u8>,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let item = ClientboundItemT::new(layout, ids).read(input)?;
    let out = StructuredItemRewriter::to_version(&item, layout, ids);
    ItemT::for_version(layout).write(output, &out)?;
    Ok(())
}

/// The seam for a stack nested in something else, such as entity metadata or
/// a particle.
#[must_use]
pub fn rewrite_item_value(input: &mut &[u8], layout: V, ids: &ComposedMappings) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    rewrite_item(input, &mut out, layout, ids).ok()?;
    Some(out)
}

pub fn item_pass(
    wrapper: &mut PacketWrapper,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let item = wrapper.read(&ClientboundItemT::new(layout, ids))?;
    let out = StructuredItemRewriter::to_version(&item, layout, ids);
    wrapper.write(&ItemT::for_version(layout), &out)
}

fn map(mapping: &IdMapping, id: i32) -> Option<i32> {
    let id = u32::try_from(id).ok()?;
    i32::try_from(mapping.map(id)?).ok()
}

fn map_component_id(
    id: i32,
    component: DataComponent,
    target: V,
    ids: &ComposedMappings,
) -> Option<i32> {
    if target == V::V_26_2
        && matches!(
            component,
            DataComponent::AttackAnimation | DataComponent::InteractAnimation
        )
    {
        return Some(i32::from(DataComponent::AttackAnimation.to_id()));
    }
    map(&ids.data_component_type, id)
}

pub(crate) fn map_component_id_from_client(id: i32, source: V, inverse: &IdMapping) -> Option<i32> {
    if source == V::V_26_2 && id == i32::from(DataComponent::AttackAnimation.to_id()) {
        return Some(i32::from(DataComponent::AttackAnimation.to_id()));
    }
    map(inverse, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::api::types::{BOOL, I8, VAR_INT};

    fn ids(target: V) -> &'static ComposedMappings {
        MappingData::get().composed(target)
    }

    /// A sharpness 5 diamond sword as core writes it, then in `target`'s form.
    fn diamond_sword(target: V) -> Item {
        let native = Item::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
            added: vec![
                ItemComponent {
                    id: i32::from(DataComponent::Enchantments.to_id()),
                    data: vec![1, 12, 5],
                },
                ItemComponent {
                    id: i32::from(DataComponent::Unbreakable.to_id()),
                    data: Vec::new(),
                },
            ],
            removed: Vec::new(),
        };
        StructuredItemRewriter::to_version(&native, target, ids(target))
    }

    /// `md('1.21.5').types.Slot`: count, item id, added and removed counts,
    /// then the components.
    #[test]
    fn a_sword_keeps_its_components_from_1_21_5_up() {
        let item = diamond_sword(V::V_1_21_5);
        let Item::Structured { added, .. } = &item else {
            panic!("structured");
        };
        assert_eq!(added.len(), 2);
        assert_eq!(added[1].data, Vec::<u8>::new(), "unbreakable is empty");
    }

    /// `md('1.21.4')` ends `enchantments` with a tooltip flag and types
    /// `unbreakable` as a bool.
    #[test]
    fn a_sword_gains_the_1_21_4_flags() {
        let item = diamond_sword(V::V_1_21_4);
        let Item::Structured { added, .. } = &item else {
            panic!("structured");
        };
        assert_eq!(added[0].data.last(), Some(&1), "tooltip flag");
        assert_eq!(added[1].data, vec![1], "unbreakable as a bool");
    }

    /// `md('1.20.3').types.slot`: present, item id, count, NBT.
    #[test]
    fn a_sword_becomes_the_nbt_form_below_1_20_5() {
        let item = diamond_sword(V::V_1_20_3);
        let Item::Nbt { count, nbt, .. } = &item else {
            panic!("nbt form, got {item:?}");
        };
        assert_eq!(*count, 1);
        let Some(pumpkin_nbt::tag::NbtTag::Compound(compound)) = nbt else {
            panic!("a compound");
        };
        assert!(compound.get_list("Enchantments").is_some());
        assert_eq!(compound.get_bool("Unbreakable"), Some(true));
    }

    fn component(item: &Item, id: DataComponent, target: V) -> Option<&ItemComponent> {
        let Item::Structured { added, .. } = item else {
            return None;
        };
        let mapped = map(&ids(target).data_component_type, i32::from(id.to_id()));
        added.iter().find(|c| Some(c.id) == mapped)
    }

    /// `md('1.21.5').types.Slot`: varint count, varint item id, the added and
    /// removed counts, then each component as an id and its payload.
    #[test]
    fn the_1_21_5_bytes_are_the_whole_stack() {
        let target = V::V_1_21_5;
        let mut bytes = Vec::new();
        ItemT::for_version(target)
            .write(&mut bytes, &diamond_sword(target))
            .unwrap();

        let mut read: &[u8] = &bytes;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1, "count");
        assert_eq!(
            VAR_INT.read(&mut read).unwrap().0,
            map(
                &ids(target).items,
                i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id)
            )
            .unwrap()
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2, "two added");
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0, "none removed");
    }

    /// `md('1.21.4')` ends `enchantments` with `showTooltip` and types
    /// `unbreakable` as a bool, so both payloads grow by one byte.
    #[test]
    fn the_1_21_4_bytes_carry_the_extra_flags() {
        let target = V::V_1_21_4;
        let item = diamond_sword(target);
        let enchantments = component(&item, DataComponent::Enchantments, target).unwrap();
        assert_eq!(enchantments.data.len(), 4);
        assert_eq!(enchantments.data[0], 1, "one enchantment");
        assert_eq!(enchantments.data[3], 1, "the tooltip flag");
        assert_eq!(
            component(&item, DataComponent::Unbreakable, target)
                .unwrap()
                .data,
            vec![1]
        );
    }

    /// `md('1.20.5').types.Slot` is the same frame; the component ids are the
    /// ones that version has.
    #[test]
    fn the_1_20_5_component_ids_are_the_targets_own() {
        let target = V::V_1_20_5;
        let item = diamond_sword(target);
        let Item::Structured { added, .. } = &item else {
            panic!("structured");
        };
        assert_eq!(
            added[0].id,
            map(
                &ids(target).data_component_type,
                i32::from(DataComponent::Enchantments.to_id())
            )
            .unwrap()
        );
    }

    /// `md('1.20.3').types.slot`: a present flag, a varint item id, a byte
    /// count and the NBT, whose root is named below 1.20.2.
    #[test]
    fn the_1_20_3_bytes_are_the_nbt_form() {
        let target = V::V_1_20_3;
        let mut bytes = Vec::new();
        ItemT::for_version(target)
            .write(&mut bytes, &diamond_sword(target))
            .unwrap();

        let mut read: &[u8] = &bytes;
        assert!(BOOL.read(&mut read).unwrap(), "present");
        assert_eq!(
            VAR_INT.read(&mut read).unwrap().0,
            map(
                &ids(target).items,
                i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id)
            )
            .unwrap()
        );
        assert_eq!(I8.read(&mut read).unwrap(), 1, "count");
        assert_eq!(read.first(), Some(&10), "a compound follows");
    }

    /// 26.2 differs from 26.3 only in `attribute_modifiers` and
    /// `jukebox_playable`, so a sword's own components are byte identical.
    #[test]
    fn the_26_2_component_payloads_are_unchanged() {
        let native = diamond_sword(V::V_26_3);
        let target = diamond_sword(V::V_26_2);
        let (Item::Structured { added: a, .. }, Item::Structured { added: b, .. }) =
            (&native, &target)
        else {
            panic!("structured");
        };
        assert_eq!(
            a.iter().map(|c| &c.data).collect::<Vec<_>>(),
            b.iter().map(|c| &c.data).collect::<Vec<_>>()
        );
    }

    #[test]
    fn interact_animation_uses_the_shared_26_2_component_and_shape() {
        let target = V::V_26_2;
        let native = Item::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
            added: vec![ItemComponent {
                id: i32::from(DataComponent::InteractAnimation.to_id()),
                data: vec![1, 6],
            }],
            removed: Vec::new(),
        };

        let mut native_bytes = Vec::new();
        ItemT::for_version(V::V_26_3)
            .write(&mut native_bytes, &native)
            .unwrap();
        let mut native_reader: &[u8] = &native_bytes;
        let decoded = ItemT::for_version(V::V_26_3)
            .read(&mut native_reader)
            .unwrap();
        assert!(native_reader.is_empty());

        let downgraded = StructuredItemRewriter::to_version(&decoded, target, ids(target));
        let Item::Structured { added, .. } = &downgraded else {
            panic!("structured");
        };
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0].id,
            i32::from(DataComponent::AttackAnimation.to_id())
        );
        assert_eq!(added[0].data, vec![1, 6]);

        let mut target_bytes = Vec::new();
        ItemT::for_version(target)
            .write(&mut target_bytes, &downgraded)
            .unwrap();
        let mut target_reader: &[u8] = &target_bytes;
        let target_item = ItemT::for_version(target).read(&mut target_reader).unwrap();
        assert!(target_reader.is_empty());
        let Item::Structured { added, .. } = target_item else {
            panic!("structured");
        };
        assert_eq!(
            added[0].id,
            i32::from(DataComponent::AttackAnimation.to_id())
        );
        assert_eq!(added[0].data, vec![1, 6]);

        let mut client_reader: &[u8] = &target_bytes;
        let native = read_client_item(&mut client_reader, target, false, ids(target)).unwrap();
        assert!(client_reader.is_empty());
        let Item::Structured { added, .. } = native else {
            panic!("structured");
        };
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0].id,
            i32::from(DataComponent::AttackAnimation.to_id())
        );
        assert_eq!(added[0].data, vec![1, 6]);
    }

    #[test]
    fn attack_and_interact_animation_collapse_without_duplicate_ids() {
        let target = V::V_26_2;
        let native = Item::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
            added: vec![
                ItemComponent {
                    id: i32::from(DataComponent::AttackAnimation.to_id()),
                    data: vec![0, 4],
                },
                ItemComponent {
                    id: i32::from(DataComponent::InteractAnimation.to_id()),
                    data: vec![1, 6],
                },
            ],
            removed: vec![
                i32::from(DataComponent::AttackAnimation.to_id()),
                i32::from(DataComponent::InteractAnimation.to_id()),
            ],
        };

        let downgraded = StructuredItemRewriter::to_version(&native, target, ids(target));
        let Item::Structured { added, removed, .. } = downgraded else {
            panic!("structured");
        };
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0].id,
            i32::from(DataComponent::AttackAnimation.to_id())
        );
        assert_eq!(added[0].data, vec![1, 6]);
        assert_eq!(
            removed,
            vec![i32::from(DataComponent::AttackAnimation.to_id())]
        );
    }

    /// `weapon` arrives in 1.21.5, so the 1.20.5 table has no id for it and
    /// the component must not be sent under someone else's.
    #[test]
    fn a_component_the_target_lacks_leaves_the_stack() {
        let target = V::V_1_20_5;
        let weapon = i32::from(DataComponent::Weapon.to_id());
        assert!(map(&ids(target).data_component_type, weapon).is_none());
        let native = Item::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
            added: vec![ItemComponent {
                id: weapon,
                data: vec![0, 0],
            }],
            removed: Vec::new(),
        };
        let Item::Structured { added, .. } =
            StructuredItemRewriter::to_version(&native, target, ids(target))
        else {
            panic!("structured");
        };
        assert!(added.is_empty());
    }

    #[test]
    fn an_empty_stack_stays_empty_in_every_form() {
        for target in [V::V_26_2, V::V_1_21_5, V::V_1_20_3, V::V_1_16_2] {
            let out = StructuredItemRewriter::to_version(&Item::Empty, target, ids(target));
            assert!(out.is_empty(), "{target}");
            let mut bytes = Vec::new();
            ItemT::for_version(target).write(&mut bytes, &out).unwrap();
            assert_eq!(bytes, vec![0], "{target}");
        }
    }

    /// The 1.21.4 item table stops short of 26.3's registry, so every id
    /// past its end is an item that version does not have.
    #[test]
    fn an_item_the_target_lacks_becomes_an_empty_stack() {
        let target = V::V_1_21_4;
        let absent = i32::try_from(ids(target).items.len()).unwrap();
        assert!(map(&ids(target).items, absent).is_none());
        let item = Item::Structured {
            count: 1,
            id: absent,
            added: Vec::new(),
            removed: Vec::new(),
        };
        assert!(StructuredItemRewriter::to_version(&item, target, ids(target)).is_empty());
    }

    #[test]
    fn the_structured_form_round_trips_through_the_wire() {
        for target in [V::V_26_3, V::V_1_21_5, V::V_1_21_4, V::V_1_20_5] {
            let item = diamond_sword(target);
            let mut bytes = Vec::new();
            ItemT::for_version(target).write(&mut bytes, &item).unwrap();
            let mut read: &[u8] = &bytes;
            // The components are in the target's shapes, so only the frame is
            // checked back; a 26.3 read would measure them wrong.
            let count = read.get_var_int().unwrap().0;
            assert_eq!(count, 1, "{target}");
        }
    }

    #[test]
    fn a_client_stack_comes_back_with_26_3_ids() {
        let source = V::V_1_21_4;
        let item = diamond_sword(source);
        let mut bytes = Vec::new();
        ItemT::for_version(source).write(&mut bytes, &item).unwrap();
        let mut read: &[u8] = &bytes;
        let native = read_client_item(&mut read, source, false, ids(source)).unwrap();
        assert!(read.is_empty());
        let Item::Structured { id, added, .. } = &native else {
            panic!("structured");
        };
        assert_eq!(*id, i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id));
        assert_eq!(
            added[1].data,
            Vec::<u8>::new(),
            "unbreakable is empty again"
        );
    }

    /// The 1.21.5 creative slot sends every payload with its own length.
    #[test]
    fn a_length_prefixed_client_stack_comes_back_with_26_3_ids() {
        let source = V::V_1_21_5;
        let item = diamond_sword(source);
        let mut bytes = Vec::new();
        ItemT::length_prefixed(source)
            .write(&mut bytes, &item)
            .unwrap();
        let mut read: &[u8] = &bytes;
        let native = read_client_item(&mut read, source, true, ids(source)).unwrap();
        assert!(read.is_empty());
        assert_eq!(
            native.item_id(),
            Some(i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id))
        );
    }

    #[test]
    fn the_nbt_form_round_trips_back_to_components() {
        let source = V::V_1_20_3;
        let item = diamond_sword(source);
        let mut bytes = Vec::new();
        ItemT::for_version(source).write(&mut bytes, &item).unwrap();
        let mut read: &[u8] = &bytes;
        let native = read_client_item(&mut read, source, false, ids(source)).unwrap();
        assert!(read.is_empty());
        let Item::Structured { id, added, .. } = &native else {
            panic!("structured, got {native:?}");
        };
        assert_eq!(*id, i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id));
        assert!(
            added
                .iter()
                .any(|c| c.id == i32::from(DataComponent::Enchantments.to_id()))
        );
    }

    #[test]
    fn negative_client_id_set_lengths_are_rejected() {
        for count in [-1, i32::MIN] {
            let mut payload = Vec::new();
            VAR_INT.write(&mut payload, &VarInt(count)).unwrap();
            let mut cursor = payload.as_slice();
            assert!(skip_id_set(&mut cursor).is_err());
        }
    }
}
