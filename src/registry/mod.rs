//! Version specific registry data.
//!
//! A 26.3 server sends registry entries whose NBT an older client cannot decode,
//! and the client rejects the entire registry load rather than the offending
//! entry. [`generated`] holds the registry contents for each supported version,
//! produced from that version's datapack by `tools/registry-codegen`.

pub mod generated;

use std::io::Cursor;

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::{NbtReadHelper, NbtReadHelperJava};
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

/// One entry as it arrived from the server: its id and its raw NBT, if any.
struct IncomingEntry<'a> {
    entry_id: String,
    data: Option<&'a [u8]>,
}

/// Splits a `REGISTRY_DATA` payload into its registry id and the rest.
fn read_registry_id(payload: &[u8]) -> Option<(String, &[u8])> {
    let mut cursor: &[u8] = payload;
    let registry_id: String = cursor.get_str().ok()?.into();
    Some((registry_id, cursor))
}

/// Walks each entry's NBT tag rather than reading a document, since the root isn't
/// always a compound (`block_transformer` entries are lists).
fn read_entries(mut cursor: &[u8]) -> Option<Vec<IncomingEntry<'_>>> {
    let count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;

    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let entry_id: String = cursor.get_str().ok()?.into();
        let has_data = cursor.get_bool().ok()?;
        let data = if has_data {
            let mut nbt_cursor = Cursor::new(cursor);
            let mut reader = NbtReadHelperJava::new(&mut nbt_cursor);
            let tag_id = reader.get_u8().ok()?;
            NbtTag::skip_data(&mut reader, tag_id).ok()?;
            let used = usize::try_from(nbt_cursor.position()).ok()?;
            let (data, rest) = cursor.split_at(used);
            cursor = rest;
            Some(data)
        } else {
            None
        };
        entries.push(IncomingEntry { entry_id, data });
    }
    cursor.is_empty().then_some(entries)
}

/// Rewrites a `REGISTRY_DATA` payload for `version`. Entries the datapack knows are
/// replaced with that version's NBT; unknown entries keep their slot (the server
/// refers to them by position) but get a stand-in, since one undecodable entry
/// fails the whole registry load.
///
/// `None` means no registry data for `version`; `Some(None)` means the version has
/// no such registry, so the packet should be dropped.
#[must_use]
#[allow(clippy::option_option)]
pub fn build_registry_payload(
    version: JavaMinecraftVersion,
    payload: &[u8],
) -> Option<Option<Vec<u8>>> {
    let registries = generated::get_synced(version)?;
    let (registry_id, rest) = read_registry_id(payload)?;

    let wanted = registry_id
        .strip_prefix("minecraft:")
        .unwrap_or(&registry_id);

    let Some(registry) = registries.iter().find(|r| r.registry_id == wanted) else {
        return Some(None);
    };

    let incoming = read_entries(rest)?;

    // plains is the least surprising stand-in for an unknown biome
    let fallback = registry
        .entries
        .iter()
        .find(|e| e.name == "plains")
        .or_else(|| registry.entries.first())?;

    let mut buf = Vec::with_capacity(payload.len());
    buf.write_string(&registry_id).ok()?;
    buf.write_var_int(&VarInt(i32::try_from(incoming.len()).ok()?))
        .ok()?;

    for entry in &incoming {
        let bare = entry
            .entry_id
            .strip_prefix("minecraft:")
            .unwrap_or(&entry.entry_id);
        let known = registry.entries.iter().find(|e| e.name == bare);

        buf.write_string(&entry.entry_id).ok()?;
        match (known, entry.data) {
            (Some(known), _) => {
                buf.write_bool(true).ok()?;
                buf.extend_from_slice(known.data);
            }
            (None, Some(_)) => {
                buf.write_bool(true).ok()?;
                buf.extend_from_slice(fallback.data);
            }
            (None, None) => buf.write_bool(false).ok()?,
        }
    }

    Some(Some(buf))
}

/// Builds the single-compound `REGISTRY_DATA` payload that 1.20.2 to 1.20.4
/// clients receive out of the per registry packets 1.20.5 and up get.
///
/// Does for those versions what [`build_registry_payload`] does for the per registry
/// packets: unknown registries are left out, known entries carry this version's own NBT.
/// `None` means no registry data for `version` or a packet failed to parse; the caller
/// drops the whole bundle rather than send a partial one.
#[must_use]
pub fn bundle_registry_packets(
    version: JavaMinecraftVersion,
    packets: &[Vec<u8>],
) -> Option<Vec<u8>> {
    let mut root = NbtCompound::new();
    for payload in packets {
        let (registry_id, rest) = read_registry_id(payload)?;
        let mut values = Vec::new();
        for (index, entry) in read_entries(rest)?.iter().enumerate() {
            let mut value = NbtCompound::new();
            value.put_string("name", entry.entry_id.clone());
            value.put_int("id", i32::try_from(index).ok()?);
            value.put("element", NbtTag::Compound(NbtCompound::new()));
            values.push(NbtTag::Compound(value));
        }
        let mut body = NbtCompound::new();
        body.put_string("type", registry_id.clone());
        body.put_list("value", values);
        root.put(&registry_id, NbtTag::Compound(body));
    }

    let out = rewrite_registry_codec(version, &root)?;
    Some(pumpkin_nbt::Nbt::from(out).write_unnamed().to_vec())
}

/// The element NBT `version`'s datapack has for one `dimension_type` entry.
/// `name` may be bare or namespaced; an unknown name falls back to the overworld.
#[must_use]
pub fn dimension_type_element(version: JavaMinecraftVersion, name: &str) -> Option<&'static [u8]> {
    let registries = generated::get_synced(version)?;
    let registry = registries
        .iter()
        .find(|r| r.registry_id == "dimension_type")?;
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    registry
        .entries
        .iter()
        .find(|e| e.name == bare)
        .or_else(|| registry.entries.iter().find(|e| e.name == "overworld"))
        .or_else(|| registry.entries.first())
        .map(|entry| entry.data)
}

/// Rewrites the registry codec compound in place: unknown registries are left out,
/// kept entries keep the server's name and id but carry this version's own element NBT.
/// Shared by the 1.20.2-1.20.4 `REGISTRY_DATA` bundle and the 1.16.2-1.20.1 dimension codec.
#[must_use]
pub fn rewrite_registry_codec(
    version: JavaMinecraftVersion,
    root: &NbtCompound,
) -> Option<NbtCompound> {
    let registries = generated::get_synced(version)?;

    let mut out = NbtCompound::new();
    for (registry_key, registry_value) in &root.child_tags {
        let bare = registry_key
            .strip_prefix("minecraft:")
            .unwrap_or(registry_key);
        let Some(registry) = registries.iter().find(|r| r.registry_id == bare) else {
            continue;
        };
        let body = registry_value.extract_compound()?;
        let incoming = body.get_list("value")?;

        let fallback = registry
            .entries
            .iter()
            .find(|e| e.name == "plains")
            .or_else(|| registry.entries.first())?;

        let mut entries = Vec::with_capacity(incoming.len());
        for entry in incoming {
            let entry = entry.extract_compound()?;
            let name = entry.get_string("name")?;
            let id = entry.get_int("id")?;
            let bare_name = name.strip_prefix("minecraft:").unwrap_or(name);
            let known = registry
                .entries
                .iter()
                .find(|e| e.name == bare_name)
                .unwrap_or(fallback);

            let mut rewritten = NbtCompound::new();
            rewritten.put_string("name", name.to_string());
            rewritten.put_int("id", id);
            rewritten.put("element", read_tag(known.data)?);
            entries.push(NbtTag::Compound(rewritten));
        }

        let mut rewritten_registry = NbtCompound::new();
        rewritten_registry.put_string(
            "type",
            body.get_string("type").unwrap_or(registry_key).to_string(),
        );
        rewritten_registry.put_list("value", entries);
        out.put(registry_key, NbtTag::Compound(rewritten_registry));
    }

    Some(out)
}

/// Reads `data` as one unnamed network NBT compound, rejecting trailing bytes.
#[cfg(test)]
pub(crate) fn read_unnamed_compound(data: &[u8]) -> Option<NbtCompound> {
    match read_tag(data)? {
        NbtTag::Compound(compound) => Some(compound),
        _ => None,
    }
}

/// Reads one whole NBT tag (type id then payload, no root name), rejecting trailing bytes.
pub(crate) fn read_tag(data: &[u8]) -> Option<NbtTag> {
    let mut cursor = Cursor::new(data);
    let mut reader = NbtReadHelperJava::new(&mut cursor);
    let tag_id = reader.get_u8().ok()?;
    let tag = NbtTag::deserialize_data(&mut reader, tag_id).ok()?;
    if usize::try_from(cursor.position()).ok()? == data.len() {
        Some(tag)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_protocol::ser::NetworkWriteExt;

    fn packet(registry_id: &str, names: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.write_string(registry_id).unwrap();
        buf.write_var_int(&VarInt(i32::try_from(names.len()).unwrap()))
            .unwrap();
        for name in names {
            buf.write_string(&format!("minecraft:{name}")).unwrap();
            buf.write_bool(true).unwrap();
            buf.extend_from_slice(&[0x0a, 0x00]);
        }
        buf
    }

    #[test]
    fn the_bundle_drops_unknown_registries_and_replaces_elements() {
        let version = JavaMinecraftVersion::V_1_20_3;
        let out = bundle_registry_packets(
            version,
            &[
                packet("minecraft:worldgen/biome", &["plains", "pale_garden"]),
                packet("minecraft:wolf_variant", &["pale"]),
            ],
        )
        .expect("bundle built");
        let rewritten = read_unnamed_compound(&out).expect("output parses");

        assert!(
            rewritten.get("minecraft:wolf_variant").is_none(),
            "a registry the version lacks is left out"
        );
        let biome = rewritten
            .get_compound("minecraft:worldgen/biome")
            .expect("biome registry kept");
        assert_eq!(biome.get_string("type"), Some("minecraft:worldgen/biome"));
        let values = biome.get_list("value").expect("value list");
        assert_eq!(values.len(), 2, "entry count and order kept");

        let plains = read_tag(
            generated::get_synced(version)
                .expect("1.20.3 registry data")
                .iter()
                .find(|r| r.registry_id == "worldgen/biome")
                .expect("biome registry generated")
                .entries
                .iter()
                .find(|e| e.name == "plains")
                .expect("plains generated")
                .data,
        )
        .expect("plains nbt parses");

        for (index, name) in ["minecraft:plains", "minecraft:pale_garden"]
            .into_iter()
            .enumerate()
        {
            let value = values[index].extract_compound().expect("entry compound");
            assert_eq!(value.get_string("name"), Some(name), "name kept");
            assert_eq!(
                value.get_int("id"),
                Some(i32::try_from(index).unwrap()),
                "id kept"
            );
            assert_eq!(
                value.get("element"),
                Some(&plains),
                "this version's own NBT"
            );
        }
    }

    #[test]
    fn a_packet_that_does_not_parse_drops_the_bundle() {
        let mut extra = packet("minecraft:worldgen/biome", &["plains"]);
        extra.push(0);
        assert!(
            bundle_registry_packets(JavaMinecraftVersion::V_1_20_3, &[extra]).is_none(),
            "a bundle is built whole or not at all"
        );
    }

    #[test]
    fn a_version_without_registry_data_drops_the_bundle() {
        assert!(
            bundle_registry_packets(
                JavaMinecraftVersion::V_1_16_1,
                &[packet("minecraft:worldgen/biome", &["plains"])],
            )
            .is_none()
        );
    }
}
