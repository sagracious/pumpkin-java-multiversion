use std::collections::{HashMap, HashSet, VecDeque};

use crc_fast::{CrcAlgorithm::Crc32Iscsi, Digest};
use pumpkin_data::data_component::DataComponent;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::{
    codec::data_component,
    ser::{NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt},
};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::item::{map_component_id, map_component_id_from_client};
use crate::api::rewriter::item_nbt;
use crate::api::types::{HashedItem, Item, ItemComponent};
use crate::api::{ComposedMappings, IdMapping, UserConnection};

const BACKUP_KEY: &str = "PJM|26_3_backup";
const CACHE_LIMIT: usize = 1024;

#[derive(Default)]
pub struct ItemBackupCache {
    entries: HashMap<(i32, i32), Option<ItemBackup>>,
    order: VecDeque<(i32, i32)>,
}

#[derive(Clone, PartialEq)]
struct ItemBackup {
    server_item_id: i32,
    server_has_custom_data: bool,
    marker: NbtCompound,
    restore_added: Vec<ItemComponent>,
    restore_added_hashes: Vec<(i32, i32)>,
    restore_removed: Vec<i32>,
    remove_server_added: Vec<i32>,
    remove_server_removed: Vec<i32>,
}

impl ItemBackupCache {
    fn insert(&mut self, key: (i32, i32), backup: ItemBackup) {
        if let Some(existing) = self.entries.get(&key) {
            if existing.as_ref() != Some(&backup) {
                // A CRC32C collision must never select one item's backup for
                // another. Tombstone the key until it leaves the bounded cache.
                self.entries.insert(key, None);
            }
            self.order.retain(|existing| *existing != key);
            self.order.push_back(key);
            self.evict_overflow();
            return;
        }
        self.entries.insert(key, Some(backup));
        self.order.push_back(key);
        self.evict_overflow();
    }

    fn evict_overflow(&mut self) {
        while self.order.len() > CACHE_LIMIT {
            if let Some(expired) = self.order.pop_front() {
                self.entries.remove(&expired);
            }
        }
    }

    fn get(&self, key: (i32, i32)) -> Option<&ItemBackup> {
        self.entries.get(&key).and_then(Option::as_ref)
    }
}

/// Adds a client-visible marker to the downgraded item's custom data and
/// remembers fields that need server-side recovery. The marker is accepted
/// only when a matching entry exists in this connection's bounded cache.
pub fn backup_clientbound_item(
    connection: &mut UserConnection,
    original: &Item,
    downgraded: &mut Item,
    target: V,
    ids: &ComposedMappings,
) {
    if target >= V::V_26_3 {
        return;
    }

    let Some(backup) = build_backup(original, downgraded, target, ids) else {
        return;
    };
    if backup.restore_added.is_empty()
        && backup.restore_removed.is_empty()
        && backup.remove_server_added.is_empty()
        && backup.remove_server_removed.is_empty()
    {
        return;
    }

    let cache = if connection.get::<ItemBackupCache>().is_none() {
        connection.put(ItemBackupCache::default());
        connection
            .get_mut::<ItemBackupCache>()
            .expect("just inserted")
    } else {
        connection
            .get_mut::<ItemBackupCache>()
            .expect("checked above")
    };

    let Some((client_item_id, custom_hash)) = add_marker(downgraded, &backup.marker, target, ids)
    else {
        return;
    };
    cache.insert((client_item_id, custom_hash), backup);
}

/// Rewrites a client hash item into Pumpkin's native component ids and hash
/// conventions, restoring cached 26.3 components when its marker matches.
pub fn rewrite_hashed_item(
    connection: &UserConnection,
    item: &mut HashedItem,
    source: V,
    ids: &ComposedMappings,
) -> Option<()> {
    let client_item_id = item.id;
    let custom_data_client_id = map_component_id(
        i32::from(DataComponent::CustomData.to_id()),
        DataComponent::CustomData,
        source,
        ids,
    );
    let custom_data_hash = custom_data_client_id.and_then(|custom_id| {
        item.added
            .iter()
            .find(|(id, _)| *id == custom_id)
            .map(|(_, hash)| *hash)
    });
    let cached = custom_data_hash.and_then(|hash| {
        connection
            .get::<ItemBackupCache>()
            .and_then(|cache| cache.get((client_item_id, hash)).cloned())
    });

    item.id = map(ids.items_inverse(), item.id)?;
    item.added.retain(|(id, _)| {
        map_component_id_from_client(*id, source, ids.data_component_type_inverse()).is_some()
    });
    for (id, hash) in &mut item.added {
        *id = map_component_id_from_client(*id, source, ids.data_component_type_inverse())?;
        if *id == i32::from(DataComponent::CustomData.to_id()) {
            // Pumpkin's CustomDataImpl currently reports zero from get_hash.
            *hash = 0;
        }
    }
    item.removed = item
        .removed
        .iter()
        .filter_map(|id| {
            map_component_id_from_client(*id, source, ids.data_component_type_inverse())
        })
        .collect();

    if let Some(backup) = cached.filter(|backup| backup.server_item_id == item.id) {
        item.added
            .retain(|(id, _)| !backup.remove_server_added.contains(id));
        if !backup.server_has_custom_data {
            item.added
                .retain(|(id, _)| *id != i32::from(DataComponent::CustomData.to_id()));
        }
        item.removed
            .retain(|id| !backup.remove_server_removed.contains(id));
        item.added
            .extend(backup.restore_added_hashes.iter().copied());
        item.removed.extend(backup.restore_removed.iter().copied());
    }
    Some(())
}

/// Removes a PJM marker and restores its source components for full-stack
/// serverbound packets such as creative-slot updates.
pub fn restore_full_item(
    connection: &UserConnection,
    item: &mut Item,
    source: V,
    ids: &ComposedMappings,
) {
    if source >= V::V_26_3 {
        return;
    }
    let Item::Structured {
        id, added, removed, ..
    } = item
    else {
        return;
    };

    let Some(client_item_id) = map(&ids.items, *id) else {
        return;
    };
    let custom_data_id = i32::from(DataComponent::CustomData.to_id());
    let Some(custom_data) = added
        .iter()
        .find(|component| component.id == custom_data_id)
    else {
        return;
    };
    let Some(mut compound) = read_custom_data(&custom_data.data) else {
        return;
    };
    let Some(custom_hash) = hash_compound(&compound) else {
        return;
    };
    let Some(backup) = connection
        .get::<ItemBackupCache>()
        .and_then(|cache| cache.get((client_item_id, custom_hash)).cloned())
        .filter(|backup| backup.server_item_id == *id)
    else {
        return;
    };
    if !matches!(
        compound.get(BACKUP_KEY),
        Some(NbtTag::Compound(marker)) if marker == &backup.marker
    ) {
        return;
    }

    compound.child_tags.remove(BACKUP_KEY);
    if compound.is_empty() && !backup.server_has_custom_data {
        added.retain(|component| component.id != custom_data_id);
    } else if let Some(data) = write_custom_data(&compound) {
        if let Some(component) = added
            .iter_mut()
            .find(|component| component.id == custom_data_id)
        {
            component.data = data;
        }
    } else {
        return;
    }
    added.retain(|component| !backup.remove_server_added.contains(&component.id));
    removed.retain(|id| !backup.remove_server_removed.contains(id));
    added.extend(backup.restore_added);
    removed.extend(backup.restore_removed);
}

fn build_backup(
    original: &Item,
    downgraded: &Item,
    target: V,
    ids: &ComposedMappings,
) -> Option<ItemBackup> {
    let Item::Structured {
        id: server_item_id,
        added: source_added,
        removed: source_removed,
        ..
    } = original
    else {
        return None;
    };
    let decoded_legacy = match downgraded {
        Item::Nbt { nbt, .. } => Some(match nbt {
            Some(NbtTag::Compound(compound)) => item_nbt::nbt_to_components(compound, target),
            None => Vec::new(),
            Some(_) => return None,
        }),
        Item::Structured { .. } => None,
        Item::Empty => return None,
    };
    let legacy_nbt = decoded_legacy.is_some();
    let (client_added, client_removed): (&[ItemComponent], &[i32]) = match downgraded {
        Item::Structured { added, removed, .. } => (added, removed),
        Item::Nbt { .. } => (decoded_legacy.as_deref().unwrap_or_default(), &[]),
        Item::Empty => return None,
    };

    let mut added_groups: HashMap<i32, Vec<&ItemComponent>> = HashMap::new();
    let mut added_unmapped = Vec::new();
    for component in source_added {
        let Some(native) = component_type(component.id) else {
            return None;
        };
        let mapped = if legacy_nbt {
            Some(component.id)
        } else {
            map_component_id(component.id, native, target, ids)
        };
        if let Some(mapped) = mapped {
            added_groups.entry(mapped).or_default().push(component);
        } else {
            added_unmapped.push(component.clone());
        }
    }

    let mut removed_groups: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut removed_unmapped = Vec::new();
    for component_id in source_removed {
        let Some(native) = component_type(*component_id) else {
            return None;
        };
        if legacy_nbt {
            removed_unmapped.push(*component_id);
            continue;
        }
        if let Some(mapped) = map_component_id(*component_id, native, target, ids) {
            removed_groups
                .entry(mapped)
                .or_default()
                .push(*component_id);
        } else {
            removed_unmapped.push(*component_id);
        }
    }

    let mut restore_added = added_unmapped;
    let mut restore_removed = removed_unmapped;
    let mut remove_client_added = HashSet::new();
    let mut remove_client_removed = HashSet::new();

    for (mapped_id, group) in &added_groups {
        let cross_collision = removed_groups.contains_key(mapped_id);
        let output = client_added
            .iter()
            .find(|component| component.id == *mapped_id);
        let output_has_id = output.is_some();
        let payload_changed = group.len() == 1
            && output.is_some_and(|output| output.data.as_slice() != group[0].data.as_slice());
        let explicit_via_backup = group
            .iter()
            .any(|component| component_type(component.id).is_some_and(via_backup_component));
        if group.len() > 1
            || cross_collision
            || !output_has_id
            || payload_changed
            || explicit_via_backup
        {
            restore_added.extend(group.iter().map(|component| (**component).clone()));
            if output_has_id {
                remove_client_added.insert(*mapped_id);
            }
        }
    }
    for (mapped_id, group) in &removed_groups {
        let cross_collision = added_groups.contains_key(mapped_id);
        let output_has_id = client_removed.contains(mapped_id);
        if group.len() > 1 || cross_collision || !output_has_id {
            restore_removed.extend(group.iter().copied());
            if output_has_id {
                remove_client_removed.insert(*mapped_id);
            }
        }
    }
    restore_added.sort_by_key(|component| component.id);
    restore_removed.sort_unstable();

    let restore_added_hashes = if target >= V::V_1_21_5 {
        restore_added
            .iter()
            .map(|component| Some((component.id, component_hash(component)?)))
            .collect::<Option<Vec<_>>>()?
    } else {
        // The 1.21.5 step strips older clients' full item data when it turns
        // clicks into hashes, so these component hashes are not sent upstream.
        Vec::new()
    };

    let inverse = ids.data_component_type_inverse();
    let mut remove_server_added = Vec::new();
    for target_id in remove_client_added {
        if legacy_nbt {
            remove_server_added.push(target_id);
        } else if let Some(id) = map_component_id_from_client(target_id, target, inverse) {
            remove_server_added.push(id);
        }
        if let Some(group) = added_groups.get(&target_id) {
            remove_server_added.extend(group.iter().map(|component| component.id));
        }
    }
    let custom_model_data_id = i32::from(DataComponent::CustomModelData.to_id());
    if !source_added
        .iter()
        .any(|component| component.id == custom_model_data_id)
        && u32::try_from(*server_item_id)
            .ok()
            .is_some_and(|id| ids.custom_model_data.contains_key(&id))
    {
        let client_model_id = if legacy_nbt {
            Some(custom_model_data_id)
        } else {
            map_component_id(
                custom_model_data_id,
                DataComponent::CustomModelData,
                target,
                ids,
            )
        };
        if client_model_id.is_some_and(|id| client_added.iter().any(|component| component.id == id))
        {
            remove_server_added.push(custom_model_data_id);
        }
    }
    let mut remove_server_removed = Vec::new();
    for target_id in remove_client_removed {
        if let Some(id) = map_component_id_from_client(target_id, target, inverse) {
            remove_server_removed.push(id);
        }
        if let Some(group) = removed_groups.get(&target_id) {
            remove_server_removed.extend(group.iter().copied());
        }
    }
    remove_server_added.sort_unstable();
    remove_server_added.dedup();
    remove_server_removed.sort_unstable();
    remove_server_removed.dedup();

    let marker = marker_tag(&restore_added, &restore_removed);
    Some(ItemBackup {
        server_item_id: *server_item_id,
        server_has_custom_data: source_added
            .iter()
            .any(|component| component.id == i32::from(DataComponent::CustomData.to_id())),
        marker,
        restore_added,
        restore_added_hashes,
        restore_removed,
        remove_server_added,
        remove_server_removed,
    })
}

fn add_marker(
    item: &mut Item,
    marker: &NbtCompound,
    target: V,
    ids: &ComposedMappings,
) -> Option<(i32, i32)> {
    match item {
        Item::Structured { id, added, .. } => {
            let client_item_id = *id;
            let custom_id = map_component_id(
                i32::from(DataComponent::CustomData.to_id()),
                DataComponent::CustomData,
                target,
                ids,
            )?;
            let custom_data = if let Some(existing) =
                added.iter_mut().find(|component| component.id == custom_id)
            {
                let mut compound = read_custom_data(&existing.data)?;
                if compound.child_tags.contains_key(BACKUP_KEY) {
                    return None;
                }
                compound.put(BACKUP_KEY, NbtTag::Compound(marker.clone()));
                existing.data = write_custom_data(&compound)?;
                compound
            } else {
                let mut compound = NbtCompound::new();
                compound.put(BACKUP_KEY, NbtTag::Compound(marker.clone()));
                added.push(ItemComponent {
                    id: custom_id,
                    data: write_custom_data(&compound)?,
                });
                compound
            };
            Some((client_item_id, hash_compound(&custom_data)?))
        }
        Item::Nbt { id, nbt, .. } => {
            let compound = match nbt {
                Some(NbtTag::Compound(compound)) => compound,
                None => {
                    *nbt = Some(NbtTag::Compound(NbtCompound::new()));
                    let Some(NbtTag::Compound(compound)) = nbt.as_mut() else {
                        return None;
                    };
                    compound
                }
                Some(_) => return None,
            };
            if compound.child_tags.contains_key(BACKUP_KEY) {
                return None;
            }
            compound.put(BACKUP_KEY, NbtTag::Compound(marker.clone()));
            let custom_data = item_nbt::nbt_to_components(compound, target)
                .into_iter()
                .find(|component| component.id == i32::from(DataComponent::CustomData.to_id()))
                .and_then(|component| read_custom_data(&component.data))?;
            Some((*id, hash_compound(&custom_data)?))
        }
        Item::Empty => None,
    }
}

fn marker_tag(added: &[ItemComponent], removed: &[i32]) -> NbtCompound {
    let mut marker = NbtCompound::new();
    let entries = added
        .iter()
        .map(|component| {
            let mut value = NbtCompound::new();
            value.put_int("id", component.id);
            value.put(
                "data",
                NbtTag::ByteArray(
                    component
                        .data
                        .iter()
                        .map(|byte| *byte as i8)
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                ),
            );
            NbtTag::Compound(value)
        })
        .collect();
    marker.put("added", NbtTag::List(entries));
    marker.put("removed", NbtTag::IntArray(removed.to_vec()));
    marker
}

fn read_custom_data(data: &[u8]) -> Option<NbtCompound> {
    let mut input = data;
    let compound = input.get_compound_nbt_with_version(&V::V_26_3).ok()??;
    input.is_empty().then_some(compound)
}

fn write_custom_data(compound: &NbtCompound) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    output
        .write_nbt_with_version(Some(&NbtTag::Compound(compound.clone())), &V::V_26_3)
        .ok()?;
    Some(output)
}

fn component_type(id: i32) -> Option<DataComponent> {
    u8::try_from(id).ok().and_then(DataComponent::try_from_id)
}

fn via_backup_component(component: DataComponent) -> bool {
    matches!(
        component,
        DataComponent::AttackAnimation
            | DataComponent::InteractAnimation
            | DataComponent::SignTextFront
            | DataComponent::SignTextBack
    )
}

fn map(mapping: &IdMapping, id: i32) -> Option<i32> {
    let id = u32::try_from(id).ok()?;
    i32::try_from(mapping.map(id)?).ok()
}

fn component_hash(component: &ItemComponent) -> Option<i32> {
    let native = component_type(component.id)?;
    let mut input = component.data.as_slice();
    let value = data_component::deserialize(native, &mut input).ok()?;
    input.is_empty().then(|| value.get_hash())
}

fn hash_compound(compound: &NbtCompound) -> Option<i32> {
    let mut bytes = vec![2];
    let mut entries = Vec::with_capacity(compound.child_tags.len());
    for (key, value) in &compound.child_tags {
        entries.push((hash_string(key)? as u32, hash_tag(value)? as u32));
    }
    entries.sort_unstable();
    for (key, value) in entries {
        bytes.extend(key.to_le_bytes());
        bytes.extend(value.to_le_bytes());
    }
    bytes.push(3);
    Some(crc32c(&bytes))
}

fn hash_tag(tag: &NbtTag) -> Option<i32> {
    let mut bytes = Vec::new();
    match tag {
        NbtTag::End => return None,
        NbtTag::Byte(value) => bytes.extend([6, *value as u8]),
        NbtTag::Short(value) => {
            bytes.push(7);
            bytes.extend(value.to_le_bytes());
        }
        NbtTag::Int(value) => {
            bytes.push(8);
            bytes.extend(value.to_le_bytes());
        }
        NbtTag::Long(value) => {
            bytes.push(9);
            bytes.extend(value.to_le_bytes());
        }
        NbtTag::Float(value) => {
            bytes.push(10);
            let bits = if value.is_nan() {
                f32::NAN.to_bits()
            } else {
                value.to_bits()
            };
            bytes.extend(bits.to_le_bytes());
        }
        NbtTag::Double(value) => {
            bytes.push(11);
            let bits = if value.is_nan() {
                f64::NAN.to_bits()
            } else {
                value.to_bits()
            };
            bytes.extend(bits.to_le_bytes());
        }
        NbtTag::ByteArray(values) => {
            bytes.push(14);
            bytes.extend(values.iter().map(|value| *value as u8));
            bytes.push(15);
        }
        NbtTag::String(value) => return Some(hash_string(value)?),
        NbtTag::List(values) => {
            bytes.push(4);
            for value in values {
                bytes.extend(hash_tag(value)?.to_le_bytes());
            }
            bytes.push(5);
        }
        NbtTag::Compound(value) => return hash_compound(value),
        NbtTag::IntArray(values) => {
            bytes.push(16);
            for value in values {
                bytes.extend(value.to_le_bytes());
            }
            bytes.push(17);
        }
        NbtTag::LongArray(values) => {
            bytes.push(18);
            for value in values {
                bytes.extend(value.to_le_bytes());
            }
            bytes.push(19);
        }
    }
    Some(crc32c(&bytes))
}

fn hash_string(value: &str) -> Option<i32> {
    let units: Vec<_> = value.encode_utf16().collect();
    let len = u32::try_from(units.len()).ok()?;
    let mut bytes = Vec::with_capacity(1 + 4 + units.len() * 2);
    bytes.push(12);
    bytes.extend(len.to_le_bytes());
    for unit in units {
        bytes.extend(unit.to_le_bytes());
    }
    Some(crc32c(&bytes))
}

fn crc32c(bytes: &[u8]) -> i32 {
    let mut digest = Digest::new(Crc32Iscsi);
    digest.update(bytes);
    digest.finalize() as u32 as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::api::rewriter::item::{
        ClientboundItemT, StructuredItemRewriter, rewrite_item_value_with_connection,
    };
    use crate::api::types::{ItemT, NbtT, VAR_INT, WireType};
    use pumpkin_data::data_component_impl::{get_i32_hash, get_str_hash};
    use pumpkin_data::item::Item as DataItem;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    fn read_target_item_26_2(bytes: &[u8], ids: &ComposedMappings) -> Item {
        let version = V::V_26_2;
        let custom_data_id = map_component_id(
            i32::from(DataComponent::CustomData.to_id()),
            DataComponent::CustomData,
            version,
            ids,
        )
        .unwrap();
        let map_decorations_id = map_component_id(
            i32::from(DataComponent::MapDecorations.to_id()),
            DataComponent::MapDecorations,
            version,
            ids,
        )
        .unwrap();
        let attack_animation_id = map_component_id(
            i32::from(DataComponent::AttackAnimation.to_id()),
            DataComponent::AttackAnimation,
            version,
            ids,
        )
        .unwrap();

        let mut input = bytes;
        let count = VAR_INT.read(&mut input).unwrap().0;
        let item_id = VAR_INT.read(&mut input).unwrap().0;
        let added_count = VAR_INT.read(&mut input).unwrap().0;
        let removed_count = VAR_INT.read(&mut input).unwrap().0;
        let mut added = Vec::new();
        for _ in 0..added_count {
            let id = VAR_INT.read(&mut input).unwrap().0;
            let before = input;
            if id == custom_data_id || id == map_decorations_id {
                let nbt = NbtT::for_version(version).read(&mut input).unwrap();
                assert!(matches!(nbt, Some(NbtTag::Compound(_))));
            } else if id == attack_animation_id {
                VAR_INT.read(&mut input).unwrap();
                VAR_INT.read(&mut input).unwrap();
            } else {
                panic!("unexpected 26.2 test component {id}");
            }
            let data = before[..before.len() - input.len()].to_vec();
            added.push(ItemComponent { id, data });
        }
        let mut removed = Vec::new();
        for _ in 0..removed_count {
            removed.push(VAR_INT.read(&mut input).unwrap().0);
        }
        assert!(input.is_empty());
        Item::Structured {
            count,
            id: item_id,
            added,
            removed,
        }
    }

    #[test]
    fn crc32c_uses_the_vanilla_castagnoli_polynomial() {
        assert_eq!(crc32c(b"123456789") as u32, 0xe306_9283);
    }

    #[test]
    fn integer_and_string_component_hashes_match_pumpkin_hash_primitives() {
        assert_eq!(hash_tag(&NbtTag::Int(42)), Some(get_i32_hash(42) as i32));
        assert_eq!(
            hash_string("PJM|26_3_backup"),
            Some(get_str_hash("PJM|26_3_backup") as i32)
        );
    }

    #[test]
    fn compound_hash_does_not_depend_on_hashmap_iteration_order() {
        let mut left = NbtCompound::new();
        left.put_int("first", 1);
        left.put_int("second", 2);
        let mut right = NbtCompound::new();
        right.put_int("second", 2);
        right.put_int("first", 1);
        assert_eq!(hash_compound(&left), hash_compound(&right));
    }

    #[test]
    fn merged_animation_round_trips_for_hash_clicks_and_full_items() {
        let version = V::V_26_2;
        let ids = MappingData::get().composed(version);
        let native = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND_SWORD.id),
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
            removed: Vec::new(),
        };
        let mut downgraded = StructuredItemRewriter::to_version(&native, version, ids);
        let mut connection = UserConnection::new(17, version);
        backup_clientbound_item(&mut connection, &native, &mut downgraded, version, ids);

        let Item::Structured {
            id: client_id,
            added: client_added,
            ..
        } = &downgraded
        else {
            panic!("structured item");
        };
        let custom_data_id = map_component_id(
            i32::from(DataComponent::CustomData.to_id()),
            DataComponent::CustomData,
            version,
            ids,
        )
        .unwrap();
        let custom_data = client_added
            .iter()
            .find(|component| component.id == custom_data_id)
            .expect("backup custom data");
        let custom_compound = read_custom_data(&custom_data.data).unwrap();
        let custom_hash = hash_compound(&custom_compound).unwrap();

        let mut clicked = HashedItem {
            id: *client_id,
            count: 3,
            added: vec![
                (custom_data_id, custom_hash),
                (i32::from(DataComponent::AttackAnimation.to_id()), 12345),
            ],
            removed: Vec::new(),
        };
        rewrite_hashed_item(&connection, &mut clicked, version, ids).unwrap();
        assert_eq!(clicked.count, 3, "a split stack keeps its client count");
        assert!(
            !clicked
                .added
                .iter()
                .any(|(id, _)| *id == i32::from(DataComponent::CustomData.to_id()))
        );
        for component in match &native {
            Item::Structured { added, .. } => added,
            _ => unreachable!(),
        } {
            let expected = component_hash(component).unwrap();
            assert!(clicked.added.contains(&(component.id, expected)));
        }

        let mut client_bytes = Vec::new();
        ItemT::for_version(version)
            .write(&mut client_bytes, &downgraded)
            .unwrap();
        let client_item = read_target_item_26_2(&client_bytes, ids);
        let mut full_item = StructuredItemRewriter::to_native(&client_item, version, ids);
        let other_connection = UserConnection::new(18, version);
        restore_full_item(&other_connection, &mut full_item, version, ids);
        let Item::Structured { added, .. } = &full_item else {
            panic!("untrusted item remains structured");
        };
        let client_custom_data = added
            .iter()
            .find(|component| component.id == i32::from(DataComponent::CustomData.to_id()))
            .expect("untrusted marker remains visible");
        assert!(
            read_custom_data(&client_custom_data.data)
                .unwrap()
                .child_tags
                .contains_key(BACKUP_KEY)
        );

        full_item = StructuredItemRewriter::to_native(&client_item, version, ids);
        restore_full_item(&connection, &mut full_item, version, ids);
        let Item::Structured { added, .. } = full_item else {
            panic!("restored structured item");
        };
        assert_eq!(
            added,
            match native {
                Item::Structured { added, .. } => added,
                _ => unreachable!(),
            }
        );
    }

    #[test]
    fn inconvertible_components_round_trip_for_legacy_and_structured_clients() {
        let mut custom_data = NbtCompound::new();
        custom_data.put_string("owner", "kept".to_owned());
        let native = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND_SWORD.id),
            added: vec![
                ItemComponent {
                    id: i32::from(DataComponent::AttackAnimation.to_id()),
                    data: vec![0, 4],
                },
                ItemComponent {
                    id: i32::from(DataComponent::Damage.to_id()),
                    data: var_int_bytes(5),
                },
                ItemComponent {
                    id: i32::from(DataComponent::CustomData.to_id()),
                    data: write_custom_data(&custom_data).unwrap(),
                },
            ],
            removed: Vec::new(),
        };

        for version in [V::V_1_16_2, V::V_1_20_5] {
            let ids = MappingData::get().composed(version);
            let mut downgraded = StructuredItemRewriter::to_version(&native, version, ids);
            let mut connection = UserConnection::new(19, version);
            backup_clientbound_item(&mut connection, &native, &mut downgraded, version, ids);

            let mut wire = Vec::new();
            ItemT::for_version(version)
                .write(&mut wire, &downgraded)
                .unwrap();
            let mut input = wire.as_slice();
            let client_item = ItemT::for_version(version).read(&mut input).unwrap();
            assert!(input.is_empty(), "{version}");
            let mut returned = StructuredItemRewriter::to_native(&client_item, version, ids);
            restore_full_item(&connection, &mut returned, version, ids);
            let Item::Structured { added, .. } = returned else {
                panic!("the restored item remains structured");
            };
            assert!(added.contains(&native_component(
                DataComponent::AttackAnimation,
                vec![0, 4]
            )));
            assert!(added.contains(&native_component(DataComponent::Damage, var_int_bytes(5))));
            let restored_custom_data = added
                .iter()
                .find(|component| component.id == i32::from(DataComponent::CustomData.to_id()))
                .and_then(|component| read_custom_data(&component.data))
                .expect("the original custom data remains");
            assert_eq!(
                restored_custom_data.get_string("owner").as_deref(),
                Some("kept")
            );
            assert!(!restored_custom_data.child_tags.contains_key(BACKUP_KEY));
        }
    }

    #[test]
    fn via_custom_model_fallback_restores_the_original_new_item_and_removes_the_marker() {
        let target = V::V_26_2;
        let ids = MappingData::get().composed(target);
        let native = Item::Structured {
            count: 1,
            id: 72,
            added: Vec::new(),
            removed: Vec::new(),
        };
        let mut downgraded = StructuredItemRewriter::to_version(&native, target, ids);
        let mut connection = UserConnection::new(0x2623_00f1, target);
        backup_clientbound_item(&mut connection, &native, &mut downgraded, target, ids);

        let mut returned = StructuredItemRewriter::to_native(&downgraded, target, ids);
        restore_full_item(&connection, &mut returned, target, ids);
        let Item::Structured { id, added, .. } = returned else {
            panic!("the matched fallback resolves to its source item");
        };
        assert_eq!(id, 72);
        assert!(
            !added
                .iter()
                .any(|component| component.id == i32::from(DataComponent::CustomModelData.to_id())),
            "synthetic display metadata is removed before the stack returns to Pumpkin"
        );
        assert!(
            !added
                .iter()
                .any(|component| component.id == i32::from(DataComponent::CustomData.to_id())),
            "the temporary backup marker is removed"
        );
    }

    #[test]
    fn trim_and_instrument_components_round_trip_across_the_supported_layout_families() {
        let trim = [
            var_int_bytes(registry_id_26_3("trim_material", "iron")),
            var_int_bytes(registry_id_26_3("trim_pattern", "coast")),
        ]
        .concat();
        let instrument = var_int_bytes(registry_id_26_3("instrument", "ponder_goat_horn"));
        let trim_material = var_int_bytes(registry_id_26_3("trim_material", "redstone"));
        let native = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND_SWORD.id),
            added: vec![
                native_component(DataComponent::Trim, trim),
                native_component(DataComponent::Instrument, instrument),
                native_component(DataComponent::ProvidesTrimMaterial, trim_material),
            ],
            removed: Vec::new(),
        };

        for version in [V::V_1_16_2, V::V_1_20_5, V::V_1_21_5, V::V_26_2] {
            let ids = MappingData::get().composed(version);
            let mut downgraded = StructuredItemRewriter::to_version(&native, version, ids);
            let mut connection = UserConnection::new(20, version);
            backup_clientbound_item(&mut connection, &native, &mut downgraded, version, ids);

            let mut wire = Vec::new();
            ItemT::for_version(version)
                .write(&mut wire, &downgraded)
                .unwrap();
            let mut input = wire.as_slice();
            let mut returned = ClientboundItemT::new(version, ids)
                .read(&mut input)
                .unwrap();
            assert!(input.is_empty(), "{version}");
            restore_full_item(&connection, &mut returned, version, ids);
            let Item::Structured { added, .. } = returned else {
                panic!("the restored item remains structured");
            };
            for expected in match &native {
                Item::Structured { added, .. } => added,
                _ => unreachable!(),
            } {
                assert!(
                    added.contains(expected),
                    "{version}: component {}",
                    expected.id
                );
            }
        }
    }

    fn native_component(component: DataComponent, data: Vec<u8>) -> ItemComponent {
        ItemComponent {
            id: i32::from(component.to_id()),
            data,
        }
    }

    fn var_int_bytes(value: i32) -> Vec<u8> {
        let mut bytes = Vec::new();
        VAR_INT
            .write(&mut bytes, &pumpkin_protocol::codec::var_int::VarInt(value))
            .unwrap();
        bytes
    }

    fn registry_id_26_3(registry: &str, name: &str) -> i32 {
        pumpkin_data::registry::REGISTRY_V_26_3
            .iter()
            .find(|entry| entry.registry_id == registry)
            .unwrap()
            .entries
            .iter()
            .position(|entry| entry.name == name)
            .and_then(|id| i32::try_from(id).ok())
            .unwrap()
    }

    #[test]
    fn map_decoration_backup_round_trips_for_hash_clicks_and_full_items() {
        let version = V::V_26_2;
        let ids = MappingData::get().composed(version);
        let mut decoration = NbtCompound::new();
        decoration.put_string("type", "abandoned_camp".to_owned());
        decoration.put_int("x", -24);
        decoration.put_int("z", 31);
        decoration.put_float("rotation", 1.5);
        let mut decorations = NbtCompound::new();
        decorations.put("camp", NbtTag::Compound(decoration));
        let mut map_payload = Vec::new();
        map_payload
            .write_nbt_with_version(Some(&NbtTag::Compound(decorations)), &V::V_26_3)
            .unwrap();

        let native_component = ItemComponent {
            id: i32::from(DataComponent::MapDecorations.to_id()),
            data: map_payload,
        };
        let native = Item::Structured {
            count: 1,
            id: i32::from(DataItem::FILLED_MAP.id),
            added: vec![native_component.clone()],
            removed: Vec::new(),
        };
        let mut downgraded = StructuredItemRewriter::to_version(&native, version, ids);
        let mut connection = UserConnection::new(27, version);
        backup_clientbound_item(&mut connection, &native, &mut downgraded, version, ids);

        let Item::Structured {
            id: client_item_id,
            added: client_components,
            ..
        } = &downgraded
        else {
            panic!("downgraded item remains structured");
        };
        let map_decorations_id = map_component_id(
            i32::from(DataComponent::MapDecorations.to_id()),
            DataComponent::MapDecorations,
            version,
            ids,
        )
        .expect("map decorations has a 26.2 component id");
        let downgraded_map = client_components
            .iter()
            .find(|component| component.id == map_decorations_id)
            .expect("downgraded map decorations");
        let mut map_reader = downgraded_map.data.as_slice();
        let Some(NbtTag::Compound(downgraded_decorations)) =
            map_reader.get_nbt(&V::V_26_3).unwrap()
        else {
            panic!("downgraded map decorations remain a compound");
        };
        let entry = downgraded_decorations
            .get("camp")
            .and_then(NbtTag::extract_compound)
            .expect("map decoration entry");
        assert_eq!(entry.get_string("type").as_deref(), Some("village_plains"));
        assert_eq!(entry.get_int("x"), Some(-24));
        assert_eq!(entry.get_int("z"), Some(31));
        assert_eq!(entry.get_float("rotation"), Some(1.5));

        let custom_data_id = map_component_id(
            i32::from(DataComponent::CustomData.to_id()),
            DataComponent::CustomData,
            version,
            ids,
        )
        .unwrap();
        let custom_data = client_components
            .iter()
            .find(|component| component.id == custom_data_id)
            .expect("round-trip marker");
        let custom_hash = hash_compound(&read_custom_data(&custom_data.data).unwrap()).unwrap();
        let mut clicked = HashedItem {
            id: *client_item_id,
            count: 1,
            added: vec![(custom_data_id, custom_hash), (map_decorations_id, 123)],
            removed: Vec::new(),
        };
        rewrite_hashed_item(&connection, &mut clicked, version, ids).unwrap();
        assert!(clicked.added.contains(&(
            native_component.id,
            component_hash(&native_component).unwrap()
        )));

        let mut client_bytes = Vec::new();
        ItemT::for_version(version)
            .write(&mut client_bytes, &downgraded)
            .unwrap();
        let client_item = read_target_item_26_2(&client_bytes, ids);
        let mut full_item = StructuredItemRewriter::to_native(&client_item, version, ids);
        restore_full_item(&connection, &mut full_item, version, ids);
        let Item::Structured { added, .. } = full_item else {
            panic!("restored item remains structured");
        };
        assert_eq!(added, vec![native_component]);
    }

    #[test]
    fn backup_and_restore_preserve_preexisting_custom_data() {
        let version = V::V_26_2;
        let ids = MappingData::get().composed(version);
        let mut custom_data = NbtCompound::new();
        custom_data.put_string("owner", "server".to_owned());
        let native = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND_SWORD.id),
            added: vec![
                ItemComponent {
                    id: i32::from(DataComponent::CustomData.to_id()),
                    data: write_custom_data(&custom_data).unwrap(),
                },
                ItemComponent {
                    id: i32::from(DataComponent::InteractAnimation.to_id()),
                    data: vec![1, 6],
                },
            ],
            removed: Vec::new(),
        };
        let mut downgraded = StructuredItemRewriter::to_version(&native, version, ids);
        let mut connection = UserConnection::new(19, version);
        backup_clientbound_item(&mut connection, &native, &mut downgraded, version, ids);

        let mut bytes = Vec::new();
        ItemT::for_version(version)
            .write(&mut bytes, &downgraded)
            .unwrap();
        let mut reader = bytes.as_slice();
        let client_item = ItemT::for_version(version).read(&mut reader).unwrap();
        let mut full_item = StructuredItemRewriter::to_native(&client_item, version, ids);
        restore_full_item(&connection, &mut full_item, version, ids);

        let Item::Structured { added, .. } = full_item else {
            panic!("restored structured item");
        };
        let restored = added
            .iter()
            .find(|component| component.id == i32::from(DataComponent::CustomData.to_id()))
            .expect("preexisting custom data remains");
        let restored = read_custom_data(&restored.data).unwrap();
        assert_eq!(restored.get_string("owner"), Some("server"));
        assert!(!restored.child_tags.contains_key(BACKUP_KEY));
    }

    #[test]
    fn nested_stack_rewrite_records_and_restores_inconvertible_components() {
        let version = V::V_1_21_4;
        let ids = MappingData::get().composed(version);
        let native = Item::Structured {
            count: 1,
            id: i32::from(DataItem::DIAMOND_SWORD.id),
            added: vec![native_component(DataComponent::AttackAnimation, vec![1, 6])],
            removed: Vec::new(),
        };
        let mut server_bytes = Vec::new();
        ItemT::for_version(V::V_26_3)
            .write(&mut server_bytes, &native)
            .unwrap();
        let mut input = server_bytes.as_slice();
        let mut connection = UserConnection::new(21, version);
        let client_bytes =
            rewrite_item_value_with_connection(&mut input, version, ids, &mut connection).unwrap();
        assert!(input.is_empty());

        let mut reader = client_bytes.as_slice();
        let mut returned = ClientboundItemT::new(version, ids)
            .read(&mut reader)
            .unwrap();
        assert!(reader.is_empty());
        let Item::Structured { added, .. } = &returned else {
            panic!("nested result is structured");
        };
        let custom_data = added
            .iter()
            .find(|component| component.id == i32::from(DataComponent::CustomData.to_id()))
            .and_then(|component| read_custom_data(&component.data))
            .expect("nested stack carries the per-connection backup marker");
        assert!(custom_data.child_tags.contains_key(BACKUP_KEY));

        restore_full_item(&connection, &mut returned, version, ids);
        let Item::Structured { added, .. } = returned else {
            panic!("restored nested item is structured");
        };
        assert!(added.contains(&native_component(
            DataComponent::AttackAnimation,
            vec![1, 6]
        )));
        assert!(
            !added
                .iter()
                .any(|component| component.id == i32::from(DataComponent::CustomData.to_id())),
            "synthetic CustomData is removed when the source stack had none"
        );
    }
}
