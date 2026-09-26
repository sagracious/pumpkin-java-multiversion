//! Rewrites `UPDATE_TAGS` for the client's registries and id numbering.
//!
//! A group for a registry the client lacks is fatal rather than skipped, member ids are
//! 26.3's, and tags the client's own datapack had that 26.3 dropped are still referenced
//! by that version's registry entries, so an unresolved tag fails the whole registry load.
//! Groups the client can't resolve are dropped, members are renumbered, and missing tags
//! are added back.
//!
//! 1.16.2 to 1.16.5 send four fixed tag lists (blocks, items, fluids, entity types) with
//! no registry names on the wire; the registry map starts at 1.17. No group can be dropped
//! in the fixed shape since the client reads by position.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::remap::tag_id_remap::{
    block_id_for_version, entity_type_id_for_version, item_id_for_version,
};

struct Tag {
    name: String,
    ids: Vec<i32>,
}

struct Group {
    /// Empty on 1.16.x, where the packet carries no registry names; see `FIXED_REGISTRIES`.
    registry: String,
    tags: Vec<Tag>,
}

/// First version whose `UPDATE_TAGS` names the registry of each group.
pub const FIRST_WITH_REGISTRY_NAMES: JavaMinecraftVersion = JavaMinecraftVersion::V_1_17;

/// The four lists 1.16.2 to 1.16.5 send, in wire order. Their registries are
/// implied by position, so all four are always written back.
const FIXED_REGISTRIES: &[&str] = &["block", "item", "fluid", "entity_type"];

/// Reads the tag list that follows a registry (or, below 1.17, stands alone).
fn read_tags(cursor: &mut &[u8]) -> Option<Vec<Tag>> {
    let tag_count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    let mut tags = Vec::with_capacity(tag_count.min(4096));
    for _ in 0..tag_count {
        let name: String = cursor.get_str().ok()?.into();
        let id_count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
        let mut ids = Vec::with_capacity(id_count.min(65536));
        for _ in 0..id_count {
            ids.push(cursor.get_var_int().ok()?.0);
        }
        tags.push(Tag { name, ids });
    }
    Some(tags)
}

/// Reads the 1.16.2 to 1.16.5 shape: four tag lists and nothing else.
fn read_fixed_groups(payload: &[u8]) -> Option<Vec<Group>> {
    let mut cursor: &[u8] = payload;
    let mut groups = Vec::with_capacity(FIXED_REGISTRIES.len());
    for _ in FIXED_REGISTRIES {
        groups.push(Group {
            registry: String::new(),
            tags: read_tags(&mut cursor)?,
        });
    }
    if cursor.is_empty() {
        Some(groups)
    } else {
        None
    }
}

fn read_groups(payload: &[u8]) -> Option<Vec<Group>> {
    let mut cursor: &[u8] = payload;
    let count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    let mut groups = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        let registry: String = cursor.get_str().ok()?.into();
        groups.push(Group {
            registry,
            tags: read_tags(&mut cursor)?,
        });
    }
    if cursor.is_empty() {
        Some(groups)
    } else {
        None
    }
}

/// `None` for registries whose ids are the same on every version we translate for.
fn mapper(registry: &str) -> Option<fn(u16, JavaMinecraftVersion) -> Option<u16>> {
    match registry {
        "block" => Some(block_id_for_version),
        "item" => Some(item_id_for_version),
        "enchantment" => Some(enchantment_id_for_version),
        "entity_type" => Some(entity_type_id_for_version),
        _ => None,
    }
}

fn enchantment_id_for_version(id: u16, version: JavaMinecraftVersion) -> Option<u16> {
    let mapped = crate::api::MappingData::get()
        .composed(version)
        .enchantments
        .map(u32::from(id))?;
    u16::try_from(mapped).ok()
}

/// Tags `version`'s datapack has that 26.3 dropped, minus any the server sent anyway.
fn extra_tags(registry: &str, group: &Group, version: JavaMinecraftVersion) -> Vec<Tag> {
    crate::registry::generated::get_extra_tags(version)
        .unwrap_or(&[])
        .iter()
        .filter(|extra| extra.registry == registry)
        .map(|extra| Tag {
            name: format!("minecraft:{}", extra.tag),
            ids: extra.members.iter().copied().map(i32::from).collect(),
        })
        .filter(|extra| !group.tags.iter().any(|tag| tag.name == extra.name))
        .collect()
}

fn write_tags(
    out: &mut Vec<u8>,
    registry: &str,
    group: &Group,
    version: JavaMinecraftVersion,
) -> Option<()> {
    let map = mapper(registry);
    let extras = extra_tags(registry, group, version);
    out.write_var_int(&VarInt(
        i32::try_from(group.tags.len() + extras.len()).ok()?,
    ))
    .ok()?;
    for tag in group.tags.iter().chain(extras.iter()) {
        let ids: Vec<i32> = match map {
            Some(map) => tag
                .ids
                .iter()
                .filter_map(|&id| u16::try_from(id).ok())
                .filter_map(|id| map(id, version))
                .map(i32::from)
                .collect(),
            None => tag.ids.clone(),
        };
        out.write_string(&tag.name).ok()?;
        out.write_var_int(&VarInt(i32::try_from(ids.len()).ok()?))
            .ok()?;
        for id in ids {
            out.write_var_int(&VarInt(id)).ok()?;
        }
    }
    Some(())
}

/// `None` means no tag data for the version, or the payload didn't parse; caller sends it unchanged.
#[must_use]
pub fn rewrite_update_tags(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    let known = crate::registry::generated::get_tag_registries(version)?;

    if version < FIRST_WITH_REGISTRY_NAMES {
        let groups = read_fixed_groups(payload)?;
        let mut out = Vec::with_capacity(payload.len());
        for (registry, group) in FIXED_REGISTRIES.iter().zip(&groups) {
            write_tags(&mut out, registry, group, version)?;
        }
        return Some(out);
    }

    let groups = read_groups(payload)?;
    let kept: Vec<&Group> = groups
        .iter()
        .filter(|group| {
            let bare = group
                .registry
                .strip_prefix("minecraft:")
                .unwrap_or(&group.registry);
            known.contains(&bare)
        })
        .collect();

    let mut out = Vec::with_capacity(payload.len());
    out.write_var_int(&VarInt(i32::try_from(kept.len()).ok()?))
        .ok()?;
    for group in kept {
        let bare = group
            .registry
            .strip_prefix("minecraft:")
            .unwrap_or(&group.registry);
        out.write_string(&group.registry).ok()?;
        write_tags(&mut out, bare, group, version)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tag_list(out: &mut Vec<u8>, tags: &[(&str, &[i32])]) {
        out.write_var_int(&VarInt(i32::try_from(tags.len()).unwrap()))
            .unwrap();
        for (name, ids) in tags {
            out.write_string(name).unwrap();
            out.write_var_int(&VarInt(i32::try_from(ids.len()).unwrap()))
                .unwrap();
            for &id in *ids {
                out.write_var_int(&VarInt(id)).unwrap();
            }
        }
    }

    fn fixed_payload() -> Vec<u8> {
        let mut out = Vec::new();
        write_tag_list(&mut out, &[("minecraft:logs", &[1, 2])]);
        write_tag_list(&mut out, &[("minecraft:planks", &[3])]);
        write_tag_list(&mut out, &[("minecraft:water", &[0, 1])]);
        write_tag_list(&mut out, &[("minecraft:skeletons", &[4])]);
        out
    }

    fn read_fixed(payload: &[u8]) -> Vec<Vec<(String, Vec<i32>)>> {
        let mut cursor: &[u8] = payload;
        let mut lists = Vec::new();
        for _ in 0..4 {
            let tags = read_tags(&mut cursor).expect("tag list");
            lists.push(
                tags.into_iter()
                    .map(|tag| (tag.name, tag.ids))
                    .collect::<Vec<_>>(),
            );
        }
        assert!(cursor.is_empty(), "four lists and nothing else");
        lists
    }

    #[test]
    fn the_fixed_shape_keeps_four_lists_in_order() {
        for version in [
            JavaMinecraftVersion::V_1_16_2,
            JavaMinecraftVersion::V_1_16_3,
            JavaMinecraftVersion::V_1_16_4,
        ] {
            let out = rewrite_update_tags(&fixed_payload(), version)
                .unwrap_or_else(|| panic!("{version} rewritten"));
            let lists = read_fixed(&out);
            assert_eq!(lists.len(), 4, "{version}: no list may be dropped");
            assert_eq!(lists[2][0].0, "minecraft:water");
            assert_eq!(lists[2][0].1, vec![0, 1]);
            assert_eq!(lists[2].len(), 1);
        }
    }

    #[test]
    fn the_fixed_shape_adds_back_this_versions_own_tags() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let out = rewrite_update_tags(&fixed_payload(), version).expect("rewritten");
        let lists = read_fixed(&out);
        let extras = crate::registry::generated::get_extra_tags(version).expect("extra tags");
        for (index, registry) in FIXED_REGISTRIES.iter().enumerate() {
            let expected = 1 + extras.iter().filter(|e| e.registry == *registry).count();
            assert_eq!(
                lists[index].len(),
                expected,
                "{registry}: the tags 26.3 dropped are added back"
            );
        }
    }

    #[test]
    fn a_fixed_payload_that_does_not_parse_is_not_rewritten() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let payload = fixed_payload();
        assert!(
            rewrite_update_tags(&payload[..payload.len() - 1], version).is_none(),
            "a truncated payload is not walked"
        );
        let mut extra = payload.clone();
        extra.push(0);
        assert!(
            rewrite_update_tags(&extra, version).is_none(),
            "bytes left over mean this is not the shape we think it is"
        );
        let mut named = Vec::new();
        named.write_var_int(&VarInt(1)).unwrap();
        named.write_string("minecraft:block").unwrap();
        write_tag_list(&mut named, &[("minecraft:logs", &[1])]);
        assert!(rewrite_update_tags(&named, version).is_none());
    }

    #[test]
    fn the_named_shape_drops_registries_the_client_lacks() {
        let version = JavaMinecraftVersion::V_1_17;
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(2)).unwrap();
        payload.write_string("minecraft:block").unwrap();
        write_tag_list(&mut payload, &[("minecraft:logs", &[1, 2])]);
        payload.write_string("minecraft:damage_type").unwrap();
        write_tag_list(&mut payload, &[("minecraft:is_fire", &[0])]);

        let out = rewrite_update_tags(&payload, version).expect("rewritten");
        let mut cursor: &[u8] = &out;
        assert_eq!(cursor.get_var_int().unwrap().0, 1, "one group left");
        assert_eq!(&*cursor.get_str().unwrap(), "minecraft:block");
        read_tags(&mut cursor).expect("tag list");
        assert!(cursor.is_empty());
    }

    #[test]
    fn enchantment_tag_members_use_the_target_registry_ids() {
        use pumpkin_data::enchantment::Enchantment;

        let version = JavaMinecraftVersion::V_1_20_5;
        let breach = u16::from(Enchantment::from_name("breach").unwrap().id);
        let mapped_breach = crate::api::MappingData::get()
            .composed(version)
            .enchantments
            .map(u32::from(breach))
            .expect("Via's fallback maps breach to a target enchantment");

        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.write_string("minecraft:enchantment").unwrap();
        write_tag_list(
            &mut payload,
            &[("minecraft:exclusive_set", &[i32::from(breach)])],
        );
        let out = rewrite_update_tags(&payload, version).expect("rewritten");
        let groups = read_groups(&out).expect("named tag groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].registry, "minecraft:enchantment");
        assert_eq!(groups[0].tags.len(), 1);
        assert_eq!(
            groups[0].tags[0].ids,
            [i32::try_from(mapped_breach).unwrap()]
        );
    }
}
