//! Entity metadata serializer ids per version, from `assets/meta_data_type`.

use std::collections::HashMap;
use std::sync::OnceLock;

use pumpkin_util::version::JavaMinecraftVersion::{self, *};

/// What a 26.3 serializer id puts on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaKind {
    Byte,
    VarInt,
    VarLong,
    Float,
    String,
    Component,
    OptionalComponent,
    Item,
    Bool,
    Rotations,
    BlockPos,
    OptionalBlockPos,
    OptionalUuid,
    BlockState,
    OptionalBlockState,
    Particle,
    Particles,
    VillagerData,
    PaintingVariant,
    Vector3,
    Quaternion,
}

struct TypeFile {
    version: JavaMinecraftVersion,
    json: &'static str,
}

macro_rules! types {
    ($version:ident, $file:literal) => {
        TypeFile {
            version: $version,
            json: include_str!(concat!("../../assets/meta_data_type/", $file)),
        }
    };
}

/// Oldest first; a version without a file of its own reads the newest older one.
static FILES: &[TypeFile] = &[
    // 1.10 uses the 1.9 entity-data serializer table.
    types!(V_1_9, "1_9_meta_data_type.json"),
    // 1.11.1 and 1.12 retain the same serializer IDs; only metadata entries
    // and entity layouts change at those boundaries.
    types!(V_1_11, "1_12_meta_data_type.json"),
    types!(V_1_12, "1_12_meta_data_type.json"),
    types!(V_1_13, "1_13_meta_data_type.json"),
    types!(V_1_13_2, "1_13_2_meta_data_type.json"),
    types!(V_1_14, "1_14_meta_data_type.json"),
    // ViaVersion Types1_14 is shared by 1.15.2, 1.16 and 1.16.1.
    types!(V_1_15_2, "1_15_2_meta_data_type.json"),
    types!(V_1_16, "1_15_2_meta_data_type.json"),
    types!(V_1_16_1, "1_15_2_meta_data_type.json"),
    types!(V_1_16_2, "1_16_2_meta_data_type.json"),
    types!(V_1_17, "1_17_meta_data_type.json"),
    types!(V_1_18, "1_18_meta_data_type.json"),
    types!(V_1_19, "1_19_meta_data_type.json"),
    types!(V_1_19_3, "1_19_3_meta_data_type.json"),
    types!(V_1_19_4, "1_19_4_meta_data_type.json"),
    types!(V_1_20, "1_20_meta_data_type.json"),
    types!(V_1_20_2, "1_20_2_meta_data_type.json"),
    types!(V_1_20_3, "1_20_3_meta_data_type.json"),
    types!(V_1_20_5, "1_20_5_meta_data_type.json"),
    types!(V_1_21, "1_21_meta_data_type.json"),
    types!(V_1_21_2, "1_21_2_meta_data_type.json"),
    types!(V_1_21_4, "1_21_4_meta_data_type.json"),
    types!(V_1_21_5, "1_21_5_meta_data_type.json"),
    types!(V_1_21_6, "1_21_6_meta_data_type.json"),
    types!(V_1_21_7, "1_21_7_meta_data_type.json"),
    types!(V_1_21_9, "1_21_9_meta_data_type.json"),
    types!(V_1_21_11, "1_21_11_meta_data_type.json"),
    types!(V_26_1, "26_1_meta_data_type.json"),
    types!(V_26_2, "26_2_meta_data_type.json"),
    types!(V_26_3, "26_3_meta_data_type.json"),
];

/// 26.3 type name to the names `ViaVersion` uses for it, first match wins.
static ALIASES: &[(&str, &[&str])] = &[
    ("int", &["integer"]),
    ("component", &["text_component"]),
    ("optional_component", &["optional_text_component"]),
    ("rotations", &["rotation"]),
    ("direction", &["facing"]),
    (
        "optional_living_entity_reference",
        &["lazy_entity_reference", "optional_uuid"],
    ),
    ("particles", &["particle_list"]),
    ("optional_unsigned_int", &["optional_int"]),
    ("pose", &["entity_pose"]),
    ("vector3", &["vector_3f"]),
    ("quaternion", &["quaternion_f"]),
    ("weathering_copper_state", &["oxidation_level"]),
    ("resolvable_profile", &["profile"]),
    ("humanoid_arm", &["arm"]),
];

fn kind_for_name(name: &str) -> Option<MetaKind> {
    Some(match name {
        "byte" => MetaKind::Byte,
        "long" => MetaKind::VarLong,
        "float" => MetaKind::Float,
        "string" => MetaKind::String,
        "component" => MetaKind::Component,
        "optional_component" => MetaKind::OptionalComponent,
        "item_stack" => MetaKind::Item,
        "boolean" => MetaKind::Bool,
        "rotations" => MetaKind::Rotations,
        "block_pos" => MetaKind::BlockPos,
        "optional_block_pos" => MetaKind::OptionalBlockPos,
        "optional_living_entity_reference" => MetaKind::OptionalUuid,
        "block_state" => MetaKind::BlockState,
        "optional_block_state" => MetaKind::OptionalBlockState,
        "particle" => MetaKind::Particle,
        "particles" => MetaKind::Particles,
        "villager_data" => MetaKind::VillagerData,
        "painting_variant" => MetaKind::PaintingVariant,
        "vector3" => MetaKind::Vector3,
        "quaternion" => MetaKind::Quaternion,
        "int"
        | "direction"
        | "optional_unsigned_int"
        | "pose"
        | "sniffer_state"
        | "armadillo_state"
        | "copper_golem_state"
        | "weathering_copper_state"
        | "humanoid_arm"
        | "dye_color" => MetaKind::VarInt,
        // The variant registries, all plain ids with no mapping data of their own.
        other if other.ends_with("_variant") => MetaKind::VarInt,
        // `optional_global_pos` and `resolvable_profile`, which nothing writes.
        _ => return None,
    })
}

struct Tables {
    /// 26.3 serializer id to the id `version` uses for it.
    ids: Vec<(JavaMinecraftVersion, Vec<Option<i32>>)>,
    /// Wire serializer id in `version` to the canonical 26.3 serializer id.
    wire_to_current: Vec<(JavaMinecraftVersion, Vec<Option<i32>>)>,
    kinds: Vec<Option<MetaKind>>,
    names: HashMap<String, i32>,
}

fn parse(json: &str) -> HashMap<String, i32> {
    serde_json::from_str(json).expect("metadata type table")
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let base = parse(FILES.last().expect("a 26.3 table").json);
        let size = base.values().copied().max().unwrap_or(0) as usize + 1;
        let mut kinds = vec![None; size];
        for (name, id) in &base {
            kinds[*id as usize] = kind_for_name(name);
        }
        let (ids, wire_to_current): (Vec<_>, Vec<_>) = FILES
            .iter()
            .map(|file| {
                let target = parse(file.json);
                let mut table = vec![None; size];
                let wire_size = target.values().copied().max().unwrap_or(0) as usize + 1;
                let mut reverse = vec![None; wire_size];
                for (name, id) in &base {
                    let target_id = std::iter::once(name.as_str())
                        .chain(
                            ALIASES
                                .iter()
                                .find(|(from, _)| from == name)
                                .into_iter()
                                .flat_map(|(_, to)| to.iter().copied()),
                        )
                        .find_map(|name| target.get(name).copied());
                    table[*id as usize] = target_id;
                    if let Some(target_id) = target_id {
                        let slot = &mut reverse[target_id as usize];
                        // 1.15.2/1.16 have one optional block-state serializer
                        // where 26.3 distinguishes both forms. Treat that
                        // legacy wire type as the optional canonical form.
                        if slot.is_none() || name == "optional_block_state" {
                            *slot = Some(*id);
                        }
                    }
                }
                ((file.version, table), (file.version, reverse))
            })
            .unzip();
        Tables {
            ids,
            wire_to_current,
            kinds,
            names: base,
        }
    })
}

/// Maps a 26.3 serializer id onto `version`, `None` for a type it does not have
/// or when the version predates the oldest checked-in serializer table.
#[must_use]
pub fn meta_data_type_id_for_version(id: i32, version: JavaMinecraftVersion) -> Option<i32> {
    let tables = tables();
    let table = tables
        .ids
        .iter()
        .rev()
        .find(|(file, _)| *file <= version)
        .map(|(_, table)| table)?;
    table.get(usize::try_from(id).ok()?).copied().flatten()
}

/// Maps a 26.3 serializer name onto `version`'s serializer id.
#[must_use]
pub fn meta_data_type_id_for_name(name: &str, version: JavaMinecraftVersion) -> Option<i32> {
    let current_id = *tables().names.get(name)?;
    meta_data_type_id_for_version(current_id, version)
}

/// Maps a serializer id read from `version`'s metadata wire format back to
/// the canonical 26.3 id used by `EntityDataEntry` and the global id-pass.
/// Returns `None` below the oldest checked-in serializer table rather than
/// interpreting an old id using the latest table.
#[must_use]
pub fn canonical_meta_data_type_id_for_version(
    wire_id: i32,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    let tables = tables();
    let (oldest, _) = tables.wire_to_current.first()?;
    if version < *oldest {
        return None;
    }
    let table = tables
        .wire_to_current
        .iter()
        .rev()
        .find(|(file, _)| *file <= version)
        .map(|(_, table)| table)?;
    table.get(usize::try_from(wire_id).ok()?).copied().flatten()
}

/// What a 26.3 serializer id writes, `None` for one this never sees on the wire.
#[must_use]
pub fn meta_kind(id: i32) -> Option<MetaKind> {
    tables().kinds.get(usize::try_from(id).ok()?).copied()?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbering `ViaVersion`'s per version type tables have.
    #[test]
    fn ids_follow_the_version() {
        // component: 5 on 26.3, 5 on 1.20.3 (text_component), 4 on 1.16.2.
        assert_eq!(meta_data_type_id_for_version(5, V_26_3), Some(5));
        assert_eq!(meta_data_type_id_for_version(5, V_1_20_3), Some(5));
        assert_eq!(meta_data_type_id_for_version(5, V_1_16_2), Some(4));
        // pose: 20 on 26.3, 21 on 1.20.5, 18 on 1.16.2.
        assert_eq!(meta_data_type_id_for_version(20, V_1_20_5), Some(21));
        assert_eq!(meta_data_type_id_for_version(20, V_1_16_2), Some(18));
        // A version with no file of its own reads the newest older one.
        assert_eq!(
            meta_data_type_id_for_version(20, V_1_16_4),
            meta_data_type_id_for_version(20, V_1_16_2)
        );
        assert_eq!(
            meta_data_type_id_for_version(20, V_1_18_2),
            meta_data_type_id_for_version(20, V_1_18)
        );
    }

    #[test]
    fn legacy_serializer_ids_are_defined_for_1_15_and_1_16() {
        for version in [V_1_15_2, V_1_16, V_1_16_1] {
            assert_eq!(
                meta_data_type_id_for_name("byte", version),
                Some(0),
                "{version}"
            );
            assert_eq!(
                meta_data_type_id_for_name("int", version),
                Some(1),
                "{version}"
            );
            assert_eq!(
                meta_data_type_id_for_name("float", version),
                Some(2),
                "{version}"
            );
            assert_eq!(
                meta_data_type_id_for_name("component", version),
                Some(4),
                "{version}"
            );
            assert_eq!(
                meta_data_type_id_for_name("item_stack", version),
                Some(6),
                "{version}"
            );
            assert_eq!(
                meta_data_type_id_for_name("particle", version),
                Some(15),
                "{version}"
            );
            assert_eq!(
                meta_data_type_id_for_name("pose", version),
                Some(18),
                "{version}"
            );
        }
    }

    #[test]
    fn serializer_ids_fail_closed_before_the_oldest_checked_in_table() {
        assert_eq!(meta_data_type_id_for_version(1, V_1_8), None);
        assert_eq!(canonical_meta_data_type_id_for_version(1, V_1_8), None);
    }

    #[test]
    fn serializer_ids_follow_the_pre_1_15_boundaries() {
        assert_eq!(meta_data_type_id_for_name("item_stack", V_1_9), Some(5));
        assert_eq!(meta_data_type_id_for_name("item_stack", V_1_12), Some(5));
        assert_eq!(meta_data_type_id_for_name("item_stack", V_1_13), Some(6));
        assert_eq!(meta_data_type_id_for_name("item_stack", V_1_14), Some(6));
        assert_eq!(meta_data_type_id_for_name("villager_data", V_1_13_2), None);
        assert_eq!(
            meta_data_type_id_for_name("villager_data", V_1_14),
            Some(16)
        );
        assert_eq!(
            canonical_meta_data_type_id_for_version(13, V_1_14),
            Some(15),
            "the shared old block-state serializer is the optional form"
        );
    }

    #[test]
    fn a_type_the_version_lacks_has_no_id() {
        // particles arrived in 1.20.5 as particle_list.
        assert_eq!(meta_data_type_id_for_version(17, V_1_20_3), None);
        assert_eq!(meta_data_type_id_for_version(17, V_1_20_5), Some(18));
        // dye_color is 26.3 only.
        assert_eq!(meta_data_type_id_for_version(43, V_26_2), None);
        assert_eq!(meta_data_type_id_for_version(43, V_26_3), Some(43));
        // The sound variants arrived in 26.1.
        assert_eq!(meta_data_type_id_for_version(22, V_1_21_11), None);
        assert_eq!(meta_data_type_id_for_version(22, V_26_1), Some(22));
    }

    #[test]
    fn serializer_names_resolve_to_the_target_version() {
        let current_int = meta_data_type_id_for_name("int", V_26_3).unwrap();
        let current_long = meta_data_type_id_for_name("long", V_26_3).unwrap();
        assert_eq!(
            meta_data_type_id_for_name("int", V_1_21_9),
            meta_data_type_id_for_version(current_int, V_1_21_9)
        );
        assert_eq!(
            meta_data_type_id_for_name("long", V_1_21_9),
            meta_data_type_id_for_version(current_long, V_1_21_9)
        );
    }

    #[test]
    fn legacy_wire_ids_reverse_to_canonical_types_per_version() {
        for version in [V_1_15_2, V_1_16, V_1_16_1, V_1_16_2] {
            assert_eq!(canonical_meta_data_type_id_for_version(7, version), Some(8));
            // Wire id 8 is rotation (12 bytes), while canonical id 8 is bool.
            assert_eq!(canonical_meta_data_type_id_for_version(8, version), Some(9));
            assert_eq!(meta_kind(9), Some(MetaKind::Rotations));
            assert_eq!(meta_kind(8), Some(MetaKind::Bool));
            // The old protocol has only the optional block-state serializer.
            assert_eq!(
                canonical_meta_data_type_id_for_version(13, version),
                Some(15)
            );
        }
        assert_eq!(
            canonical_meta_data_type_id_for_version(8, V_1_14_4),
            Some(9)
        );
        assert_eq!(canonical_meta_data_type_id_for_version(-1, V_1_16), None);
    }

    #[test]
    fn kinds_come_from_the_26_3_names() {
        assert_eq!(meta_kind(0), Some(MetaKind::Byte));
        assert_eq!(meta_kind(7), Some(MetaKind::Item));
        assert_eq!(meta_kind(14), Some(MetaKind::BlockState));
        assert_eq!(meta_kind(15), Some(MetaKind::OptionalBlockState));
        assert_eq!(meta_kind(16), Some(MetaKind::Particle));
        assert_eq!(meta_kind(17), Some(MetaKind::Particles));
        assert_eq!(meta_kind(21), Some(MetaKind::VarInt));
        assert_eq!(meta_kind(34), Some(MetaKind::PaintingVariant));
        // optional_global_pos and resolvable_profile have no reader.
        assert_eq!(meta_kind(33), None);
        assert_eq!(meta_kind(41), None);
        assert_eq!(meta_kind(99), None);
    }
}
