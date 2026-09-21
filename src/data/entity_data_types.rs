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
    kinds: Vec<Option<MetaKind>>,
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
        let ids = FILES
            .iter()
            .map(|file| {
                let target = parse(file.json);
                let mut table = vec![None; size];
                for (name, id) in &base {
                    table[*id as usize] = std::iter::once(name.as_str())
                        .chain(
                            ALIASES
                                .iter()
                                .find(|(from, _)| from == name)
                                .into_iter()
                                .flat_map(|(_, to)| to.iter().copied()),
                        )
                        .find_map(|name| target.get(name).copied());
                }
                (file.version, table)
            })
            .collect();
        Tables { ids, kinds }
    })
}

/// Maps a 26.3 serializer id onto `version`, `None` for a type it does not have.
#[must_use]
pub fn meta_data_type_id_for_version(id: i32, version: JavaMinecraftVersion) -> Option<i32> {
    let tables = tables();
    let table = tables
        .ids
        .iter()
        .rev()
        .find(|(file, _)| *file <= version)
        .or_else(|| tables.ids.last())
        .map(|(_, table)| table)?;
    table.get(usize::try_from(id).ok()?).copied().flatten()
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
