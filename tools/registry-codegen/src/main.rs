//! Generates `src/registry/generated.rs` for the multiversion plugin.
//!
//! Reads `assets/datapacks/<version>/data/minecraft/<registry>/*.json` and emits
//! one `REGISTRY_V_<VERSION>` static per version, holding pre-serialized NBT for
//! every synced registry entry. The JSON to NBT conversion mirrors Pumpkin's
//! `tools/pumpkin-codegen/src/registry.rs`, so the bytes match what the server
//! itself produces for its own version.
//!
//! Run from the repository root: `cargo run --manifest-path tools/registry-codegen/Cargo.toml`

use std::fs;
use std::path::Path;

use indexmap::IndexMap;
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use serde_json::Value;

/// Registries the server sends to the client during configuration.
const SYNCED_REGISTRIES: &[&str] = &[
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_variant",
    "cow_sound_variant",
    "chicken_variant",
    "chicken_sound_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "dimension_type",
    "damage_type",
    "jukebox_song",
    "banner_pattern",
    "instrument",
    "enchantment",
    "timeline",
    "dialog",
    "world_clock",
    "test_environment",
    "test_instance",
    "sulfur_cube_archetype",
    "decorated_pot_pattern",
    "block_transformer",
    "worldgen/block_state_provider",
];

/// One generated `REGISTRY_V_*` static: which datapack folder its registries
/// and tags come from, and the per-registry exceptions.
struct VersionSpec {
    /// Datapack folder under `assets/datapacks`, also the source for the tag
    /// registry list and the extra tags.
    folder: &'static str,
    /// `JavaMinecraftVersion` variant name.
    ident: &'static str,
    /// Registries whose JSON lives in another version's folder, because this
    /// version shares that folder's shape for that one registry only.
    registry_overrides: &'static [(&'static str, &'static str)],
    /// `(registry, field)` pairs to strip from every entry of that registry:
    /// a field the source folder has and this version's codec does not.
    dropped_fields: &'static [(&'static str, &'static str)],
}

const fn spec(folder: &'static str, ident: &'static str) -> VersionSpec {
    VersionSpec {
        folder,
        ident,
        registry_overrides: &[],
        dropped_fields: &[],
    }
}

/// `chat_type` took the flat `{chat, narration}` shape in 1.19.1, three
/// releases before the biome and dimension shapes changed, so 1.19.1 and
/// 1.19.3 take their biomes and dimension types from the 1.19 datapack and
/// their chat types from the 1.20 one. Checked against the vanilla codec
/// dumps in minecraft-data (`pc/1.19` vs `pc/1.19.2` `loginPacket.json`):
/// 1.19 has the nested `decoration`/`priority` form and the eight 1.19 names,
/// 1.19.2 already has the flat form and the `*_incoming`/`*_outgoing` names.
const CHAT_TYPE_FROM_1_20: &[(&str, &str)] = &[("chat_type", "1_20")];

/// The `1_20` datapack folder holds 1.19.4's data, not 1.20's: 63 biomes and
/// 42 damage types, where vanilla 1.20 has 64 and 44, and no `trim_pattern` or
/// `trim_material` at all. Checked entry by entry against the vanilla codec
/// dumps in minecraft-data: `pc/1.19.4/loginPacket.json` matches that folder
/// exactly, `pc/1.20/loginPacket.json` is short `cherry_grove`,
/// `generic_kill`, `outside_border` and the trim registries.
///
/// So 1.19.4 takes the folder as it is and only borrows the trims, while 1.20
/// takes every registry that differs from the 1.20.2 folder, whose contents
/// are vanilla 1.20's with one field added: `decal` on a trim pattern. That
/// field is stripped rather than left for the client's codec to ignore, and
/// with it the two agree field for field on all six registries.
const TRIM_FROM_1_20_2: &[(&str, &str)] =
    &[("trim_pattern", "1_20_2"), ("trim_material", "1_20_2")];
const REGISTRIES_FROM_1_20_2: &[(&str, &str)] = &[
    ("worldgen/biome", "1_20_2"),
    ("damage_type", "1_20_2"),
    ("trim_pattern", "1_20_2"),
    ("trim_material", "1_20_2"),
];
const TRIM_DECAL: &[(&str, &str)] = &[("trim_pattern", "decal")];

const VERSIONS: &[VersionSpec] = &[
    // 1.16.2 to 1.16.5 share one layout: biomes still carry `depth`,
    // `scale` and `category`, dimension types have no `min_y`/`height`, and
    // the codec holds nothing but `dimension_type` and `worldgen/biome`
    // (minecraft-data `pc/1.16.2/loginPacket.json`).
    spec("1_16_2", "V_1_16_2"),
    spec("1_16_2", "V_1_16_3"),
    spec("1_16_2", "V_1_16_4"),
    // 1.17 added `min_y`/`height` to the dimension type.
    spec("1_17", "V_1_17"),
    spec("1_17", "V_1_17_1"),
    // 1.18 dropped `depth`/`scale` from biomes.
    spec("1_18", "V_1_18"),
    spec("1_18", "V_1_18_2"),
    // 1.19 dropped `category`, added `monster_spawn_*` and `chat_type`.
    spec("1_19", "V_1_19"),
    VersionSpec {
        folder: "1_19",
        ident: "V_1_19_1",
        registry_overrides: CHAT_TYPE_FROM_1_20,
        dropped_fields: &[],
    },
    VersionSpec {
        folder: "1_19",
        ident: "V_1_19_3",
        registry_overrides: CHAT_TYPE_FROM_1_20,
        dropped_fields: &[],
    },
    // 1.19.4 replaced biome `precipitation` with `has_precipitation` and
    // added `damage_type`, `trim_pattern` and `trim_material`.
    VersionSpec {
        folder: "1_20",
        ident: "V_1_19_4",
        registry_overrides: TRIM_FROM_1_20_2,
        dropped_fields: TRIM_DECAL,
    },
    // Tags still come from the 1_20 folder: the two have the same tag
    // registries, and the extra tags are that version's own.
    VersionSpec {
        folder: "1_20",
        ident: "V_1_20",
        registry_overrides: REGISTRIES_FROM_1_20_2,
        dropped_fields: TRIM_DECAL,
    },
    spec("1_20_2", "V_1_20_2"),
    spec("1_20_3", "V_1_20_3"),
    spec("1_20_5", "V_1_20_5"),
    spec("1_21", "V_1_21"),
    spec("1_21_2", "V_1_21_2"),
    spec("1_21_4", "V_1_21_4"),
    spec("1_21_5", "V_1_21_5"),
    spec("1_21_6", "V_1_21_6"),
    spec("1_21_7", "V_1_21_7"),
    spec("1_21_9", "V_1_21_9"),
    spec("1_21_11", "V_1_21_11"),
    spec("26_1", "V_26_1"),
    spec("26_2", "V_26_2"),
];

fn json_to_nbt_tag(v: &Value) -> pumpkin_nbt::tag::NbtTag {
    match v {
        Value::Null => pumpkin_nbt::tag::NbtTag::End,
        Value::Bool(b) => pumpkin_nbt::tag::NbtTag::Byte(i8::from(*b)),
        Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                if i >= i64::from(i32::MIN) && i <= i64::from(i32::MAX) {
                    pumpkin_nbt::tag::NbtTag::Int(i as i32)
                } else {
                    pumpkin_nbt::tag::NbtTag::Long(i)
                }
            } else if let Some(f) = num.as_f64() {
                pumpkin_nbt::tag::NbtTag::Double(f)
            } else {
                pumpkin_nbt::tag::NbtTag::Int(0)
            }
        }
        Value::String(s) => pumpkin_nbt::tag::NbtTag::String(s.clone().into()),
        Value::Array(arr) => {
            pumpkin_nbt::tag::NbtTag::List(arr.iter().map(json_to_nbt_tag).collect())
        }
        Value::Object(obj) => {
            let mut compound = pumpkin_nbt::compound::NbtCompound::new();
            for (k, val) in obj {
                compound.put(k, json_to_nbt_tag(val));
            }
            pumpkin_nbt::tag::NbtTag::Compound(compound)
        }
    }
}

fn entry_bytes(entry_data: &Value) -> Vec<u8> {
    match json_to_nbt_tag(entry_data) {
        pumpkin_nbt::tag::NbtTag::Compound(compound) => {
            pumpkin_nbt::Nbt::from(compound).write_unnamed().to_vec()
        }
        other => {
            let mut bytes = Vec::new();
            let mut writer = pumpkin_nbt::serializer::NbtWriteHelperJava::new(&mut bytes);
            let _ = other.serialize(&mut writer);
            bytes
        }
    }
}

fn process_version(version: &VersionSpec) -> TokenStream {
    let mut data: IndexMap<String, IndexMap<String, Value>> = IndexMap::new();

    for &reg_name in SYNCED_REGISTRIES {
        let folder = version
            .registry_overrides
            .iter()
            .find(|(registry, _)| *registry == reg_name)
            .map_or(version.folder, |(_, folder)| *folder);
        let reg_dir = Path::new("assets/datapacks")
            .join(folder)
            .join("data/minecraft")
            .join(reg_name);
        if !reg_dir.is_dir() {
            continue;
        }
        let mut entries = IndexMap::new();
        let mut paths: Vec<_> = fs::read_dir(&reg_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort_by_key(std::fs::DirEntry::path);

        for entry in paths {
            let path = entry.path();
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if let Ok(content) = fs::read_to_string(&path)
                && let Ok(mut val) = serde_json::from_str::<Value>(&content)
            {
                for (registry, field) in version.dropped_fields {
                    if *registry == reg_name
                        && let Some(object) = val.as_object_mut()
                    {
                        object.remove(*field);
                    }
                }
                entries.insert(stem, val);
            }
        }

        if !entries.is_empty() {
            data.insert(reg_name.to_string(), entries);
        }
    }

    // The vanilla server synthesises a "raw" chat type; datapacks do not carry it.
    if let Some(chat) = data.get_mut("chat_type") {
        chat.insert(
            "raw".to_string(),
            serde_json::json!({
                "chat": { "translation_key": "%s", "parameters": ["content"] },
                "narration": { "translation_key": "%s says %s", "parameters": ["sender", "content"] }
            }),
        );
    }

    let reg_tokens: Vec<TokenStream> = data
        .iter()
        .map(|(reg_name, entries)| {
            let entry_tokens: Vec<TokenStream> = entries
                .iter()
                .map(|(entry_name, entry_data)| {
                    let byte_literal = Literal::byte_string(&entry_bytes(entry_data));
                    quote! {
                        StaticRegistryEntry { name: #entry_name, data: #byte_literal }
                    }
                })
                .collect();
            quote! {
                StaticRegistry {
                    registry_id: #reg_name,
                    entries: &[#(#entry_tokens),*],
                }
            }
        })
        .collect();

    quote! { &[#(#reg_tokens),*] }
}

/// Registry keys that have a `tags/` folder in `ver_folder`'s datapack: the
/// registries that version's client can resolve tags for. A key is the first
/// path segment, or the first two under `worldgen/`; deeper folders are tag
/// namespaces (`block/mineable`), not registries.
fn tag_registries(ver_folder: &str) -> Vec<String> {
    let tags_dir = Path::new("assets/datapacks")
        .join(ver_folder)
        .join("data/minecraft/tags");
    let mut keys = std::collections::BTreeSet::new();
    let mut stack = vec![tags_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(rel) = path.strip_prefix(&tags_dir)
            {
                let segments: Vec<String> = rel
                    .parent()
                    .map(|p| {
                        p.components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let take = if segments.first().is_some_and(|s| s == "worldgen") {
                    2
                } else {
                    1
                };
                if segments.len() >= take {
                    keys.insert(segments[..take].join("/"));
                }
            }
        }
    }
    keys.into_iter().collect()
}

/// Registries whose tag members can be resolved to 26.3 numeric ids by name.
const STATIC_TAG_REGISTRIES: &[&str] = &["block", "item", "entity_type"];

fn static_id(registry: &str, name: &str) -> Option<u16> {
    match registry {
        "block" => pumpkin_data::Block::from_name(name).map(|b| u16::from(b.id)),
        "item" => pumpkin_data::item::Item::from_registry_key(name).map(|i| i.id),
        "entity_type" => pumpkin_data::entity::EntityType::from_name(name).map(|e| e.id),
        _ => None,
    }
}

/// Reads one tag file, following `#other` references inside the same datapack.
fn tag_members(tags_dir: &Path, registry: &str, tag: &str, depth: usize) -> Vec<u16> {
    if depth > 8 {
        return Vec::new();
    }
    let path = tags_dir.join(registry).join(format!("{tag}.json"));
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    let mut members = Vec::new();
    for value in json
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = match value {
            Value::String(s) => s.as_str(),
            Value::Object(o) => o.get("id").and_then(Value::as_str).unwrap_or(""),
            _ => "",
        };
        if let Some(nested) = name.strip_prefix('#') {
            let nested = nested.strip_prefix("minecraft:").unwrap_or(nested);
            members.extend(tag_members(tags_dir, registry, nested, depth + 1));
        } else if let Some(id) = static_id(registry, name) {
            members.push(id);
        }
    }
    members.sort_unstable();
    members.dedup();
    members
}

/// Tags in `ver_folder`'s datapack that 26.3's datapack does not have, for
/// the registries whose members can be numbered. Older registry entries still
/// reference them (`enchantable/sword` on 1.21.5), and the client fails the
/// whole registry load when a referenced tag never arrives.
fn extra_tags(ver_folder: &str) -> Vec<(String, String, Vec<u16>)> {
    let old_dir = Path::new("assets/datapacks")
        .join(ver_folder)
        .join("data/minecraft/tags");
    let new_dir = Path::new("assets/datapacks/26_3/data/minecraft/tags");
    let mut out = Vec::new();
    for &registry in STATIC_TAG_REGISTRIES {
        let reg_dir = old_dir.join(registry);
        let mut stack = vec![reg_dir.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !path.extension().is_some_and(|ext| ext == "json") {
                    continue;
                }
                let Ok(rel) = path.strip_prefix(&reg_dir) else {
                    continue;
                };
                if new_dir.join(registry).join(rel).exists() {
                    continue;
                }
                let tag = rel.with_extension("").to_string_lossy().replace('\\', "/");
                let members = tag_members(&old_dir, registry, &tag, 0);
                out.push((registry.to_string(), tag, members));
            }
        }
    }
    out.sort();
    out
}

fn main() {
    let mut statics = TokenStream::new();
    let mut match_arms = TokenStream::new();
    let mut tag_arms = TokenStream::new();
    let mut extra_arms = TokenStream::new();

    for version in VERSIONS {
        let ver_folder = version.folder;
        let ident_str = version.ident;
        if !Path::new("assets/datapacks").join(ver_folder).is_dir() {
            eprintln!("skipping {ver_folder}: no datapack folder");
            continue;
        }
        eprintln!("generating registries for {ident_str} from {ver_folder}");
        let registries = process_version(version);
        let ident = format_ident!("REGISTRY_{ident_str}");
        statics.extend(quote! {
            pub static #ident: &[StaticRegistry] = #registries;
        });
        let ver_ident = format_ident!("{ident_str}");
        match_arms.extend(quote! {
            JavaMinecraftVersion::#ver_ident => Some(#ident),
        });
        let tag_keys = tag_registries(ver_folder);
        let tag_ident = format_ident!("TAG_REGISTRIES_{ident_str}");
        statics.extend(quote! {
            pub static #tag_ident: &[&str] = &[#(#tag_keys),*];
        });
        tag_arms.extend(quote! {
            JavaMinecraftVersion::#ver_ident => Some(#tag_ident),
        });
        let extras = extra_tags(ver_folder);
        eprintln!("  {} extra tags for {ver_folder}", extras.len());
        let extra_tokens: Vec<TokenStream> = extras
            .iter()
            .map(|(registry, tag, members)| {
                quote! { ExtraTag { registry: #registry, tag: #tag, members: &[#(#members),*] } }
            })
            .collect();
        let extra_ident = format_ident!("EXTRA_TAGS_{ident_str}");
        statics.extend(quote! {
            pub static #extra_ident: &[ExtraTag] = &[#(#extra_tokens),*];
        });
        extra_arms.extend(quote! {
            JavaMinecraftVersion::#ver_ident => Some(#extra_ident),
        });
    }

    let out = quote! {
        use pumpkin_util::version::JavaMinecraftVersion;

        pub struct StaticRegistryEntry {
            pub name: &'static str,
            pub data: &'static [u8],
        }

        pub struct StaticRegistry {
            pub registry_id: &'static str,
            pub entries: &'static [StaticRegistryEntry],
        }

        /// A tag this version has that 26.3 does not, with its members as
        /// 26.3 ids.
        pub struct ExtraTag {
            pub registry: &'static str,
            pub tag: &'static str,
            pub members: &'static [u16],
        }

        #statics

        /// Returns the synced registries for `version`, or `None` when no
        /// registry data has been generated for it.
        #[must_use]
        pub const fn get_synced(
            version: JavaMinecraftVersion,
        ) -> Option<&'static [StaticRegistry]> {
            match version {
                #match_arms
                _ => None,
            }
        }

        /// Returns the registries `version` can resolve tags for, or `None`
        /// when no data has been generated for it.
        #[must_use]
        pub const fn get_tag_registries(
            version: JavaMinecraftVersion,
        ) -> Option<&'static [&'static str]> {
            match version {
                #tag_arms
                _ => None,
            }
        }

        /// Returns the tags `version` has that 26.3 does not, or `None` when
        /// no data has been generated for it.
        #[must_use]
        pub const fn get_extra_tags(
            version: JavaMinecraftVersion,
        ) -> Option<&'static [ExtraTag]> {
            match version {
                #extra_arms
                _ => None,
            }
        }
    };

    let header = "/* This file is generated. Do not edit manually. */\n";
    let code = format!("{header}{out}");
    fs::create_dir_all("src/registry").expect("create src/registry");
    fs::write("src/registry/generated.rs", code).expect("write generated.rs");
    eprintln!("wrote src/registry/generated.rs");
}
