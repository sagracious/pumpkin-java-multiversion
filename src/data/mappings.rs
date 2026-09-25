//! Decodes the vendored `ViaBackwards` mapping files on first use.

use std::io::Cursor;
use std::sync::OnceLock;

use pumpkin_nbt::Nbt;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::nbt_compress::read_gzip_compound_tag;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::version::JavaMinecraftVersion::{self, *};

struct StepFile {
    from: JavaMinecraftVersion,
    to: JavaMinecraftVersion,
    data: Option<&'static [u8]>,
}

macro_rules! step {
    ($from:ident, $to:ident) => {
        StepFile {
            from: $from,
            to: $to,
            data: None,
        }
    };
    ($from:ident, $to:ident, $file:literal) => {
        StepFile {
            from: $from,
            to: $to,
            data: Some(include_bytes!(concat!(
                "../../assets/viabackwards/data/",
                $file
            ))),
        }
    };
}

/// The step chain in `ViaBackwards` order. A step whose registries did not
/// move shares the next one's file and maps ids unchanged.
const STEPS: &[StepFile] = &[
    step!(V_26_3, V_26_2, "mappings-26.3to26.2.nbt"),
    step!(V_26_2, V_26_1, "mappings-26.2to26.1.nbt"),
    step!(V_26_1, V_1_21_11, "mappings-26.1to1.21.11.nbt"),
    step!(V_1_21_11, V_1_21_9, "mappings-1.21.11to1.21.9.nbt"),
    step!(V_1_21_9, V_1_21_7, "mappings-1.21.9to1.21.7.nbt"),
    step!(V_1_21_7, V_1_21_6, "mappings-1.21.7to1.21.6.nbt"),
    step!(V_1_21_6, V_1_21_5, "mappings-1.21.6to1.21.5.nbt"),
    step!(V_1_21_5, V_1_21_4, "mappings-1.21.5to1.21.4.nbt"),
    step!(V_1_21_4, V_1_21_2, "mappings-1.21.4to1.21.2.nbt"),
    step!(V_1_21_2, V_1_21, "mappings-1.21.2to1.21.nbt"),
    step!(V_1_21, V_1_20_5, "mappings-1.21to1.20.5.nbt"),
    step!(V_1_20_5, V_1_20_3, "mappings-1.20.5to1.20.3.nbt"),
    step!(V_1_20_3, V_1_20_2, "mappings-1.20.3to1.20.2.nbt"),
    step!(V_1_20_2, V_1_20, "mappings-1.20.2to1.20.nbt"),
    step!(V_1_20, V_1_19_4, "mappings-1.20to1.19.4.nbt"),
    step!(V_1_19_4, V_1_19_3, "mappings-1.19.4to1.19.3.nbt"),
    step!(V_1_19_3, V_1_19_1, "mappings-1.19.3to1.19.nbt"),
    step!(V_1_19_1, V_1_19),
    step!(V_1_19, V_1_18_2, "mappings-1.19to1.18.nbt"),
    step!(V_1_18_2, V_1_18),
    step!(V_1_18, V_1_17_1, "mappings-1.18to1.17.nbt"),
    step!(V_1_17_1, V_1_17),
    step!(V_1_17, V_1_16_4, "mappings-1.17to1.16.2.nbt"),
    step!(V_1_16_4, V_1_16_3),
    step!(V_1_16_3, V_1_16_2),
    // Via's 1.16.2→1.16 mapping is shared by 1.16.1, whose registry IDs are
    // unchanged from 1.16.0.
    step!(V_1_16_2, V_1_16_1, "mappings-1.16.2to1.16.nbt"),
    step!(V_1_16_1, V_1_16),
    step!(V_1_16, V_1_15_2, "mappings-1.16to1.15.nbt"),
    step!(V_1_15_2, V_1_15_1),
    step!(V_1_15_1, V_1_15),
    step!(V_1_15, V_1_14_4, "mappings-1.15to1.14.nbt"),
    step!(V_1_14_4, V_1_14_3),
    step!(V_1_14_3, V_1_14_2),
    step!(V_1_14_2, V_1_14_1),
    step!(V_1_14_1, V_1_14),
    step!(V_1_14, V_1_13_2, "mappings-1.14to1.13.2.nbt"),
];

struct StepOverride {
    from: JavaMinecraftVersion,
    key: &'static str,
    table: &'static [i32],
}

/// Tables for id spaces a step changed without the mapping file saying so.
const OVERRIDES: &[StepOverride] = &[
    // 1.19.4 split the smithing table into `legacy_smithing` (20) and
    // `smithing` (21); 1.19.3 has only the old one, at 20.
    StepOverride {
        from: V_1_19_4,
        key: "menus",
        table: &[
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 20, 21, 22,
            23,
        ],
    },
    // 1.20.2 added `generic.max_absorption` at 10 of the 14 attributes 1.20.3
    // has.
    StepOverride {
        from: V_1_20_2,
        key: "attributes",
        table: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, -1, 10, 11, 12],
    },
    // 1.17 appended `sculk_sensor` (33) to the block entity registry.
    StepOverride {
        from: V_1_17,
        key: "blockentities",
        table: &[
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32, -1,
        ],
    },
];

#[derive(Clone)]
enum Repr {
    /// No step carried this id space, so every id passes through.
    Identity,
    Bounded(usize),
    Table(Vec<i32>),
}

/// An id table for one registry, `None` where the id does not exist.
#[derive(Clone)]
pub struct IdMapping(Repr);

impl IdMapping {
    pub const IDENTITY: Self = Self(Repr::Identity);

    #[must_use]
    pub fn map(&self, id: u32) -> Option<u32> {
        match &self.0 {
            Repr::Identity => Some(id),
            Repr::Bounded(size) => ((id as usize) < *size).then_some(id),
            Repr::Table(table) => table
                .get(id as usize)
                .and_then(|mapped| u32::try_from(*mapped).ok()),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Identity => 0,
            Repr::Bounded(size) => *size,
            Repr::Table(table) => table.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn is_identity(&self) -> bool {
        !matches!(&self.0, Repr::Table(_))
    }

    /// Maps back to the id space this mapping starts from, keeping the lowest
    /// id where several map onto the same one.
    #[must_use]
    pub fn inverse(&self) -> Self {
        let Repr::Table(table) = &self.0 else {
            return self.clone();
        };
        let size = table.iter().copied().max().unwrap_or(-1) + 1;
        let Ok(size) = usize::try_from(size) else {
            return Self(Repr::Table(Vec::new()));
        };
        let mut inverse = vec![-1; size];
        for (id, mapped) in table.iter().enumerate() {
            let Ok(mapped) = usize::try_from(*mapped) else {
                continue;
            };
            if inverse[mapped] < 0 {
                inverse[mapped] = i32::try_from(id).unwrap_or(-1);
            }
        }
        Self(Repr::Table(inverse))
    }

    fn compose(&self, next: &Self) -> Self {
        match (&self.0, &next.0) {
            (Repr::Identity, _) => next.clone(),
            (_, Repr::Identity) => self.clone(),
            _ => Self(Repr::Table(
                (0..self.len())
                    .map(|id| {
                        u32::try_from(id)
                            .ok()
                            .and_then(|id| self.map(id))
                            .and_then(|id| next.map(id))
                            .and_then(|id| i32::try_from(id).ok())
                            .unwrap_or(-1)
                    })
                    .collect(),
            )),
        }
    }
}

/// A tag name and the ids it holds on the step's target version.
pub type TagEntry = (String, Vec<i32>);

/// Tags a step's target version has that the newer one dropped, per registry.
#[derive(Default)]
pub struct TagMappings {
    registries: Vec<(String, Vec<TagEntry>)>,
}

impl TagMappings {
    #[must_use]
    pub fn get(&self, registry: &str) -> &[TagEntry] {
        self.registries
            .iter()
            .find(|(name, _)| name == registry)
            .map_or(&[], |(_, tags)| tags.as_slice())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registries.is_empty()
    }
}

/// One step's tables, mapping the step's `from` ids onto its `to` ids.
pub struct StepMappings {
    pub blockstates: IdMapping,
    pub blocks: IdMapping,
    pub items: IdMapping,
    pub sounds: IdMapping,
    pub blockentities: IdMapping,
    pub entities: IdMapping,
    pub particles: IdMapping,
    pub argumenttypes: IdMapping,
    pub statistics: IdMapping,
    pub recipe_serializers: IdMapping,
    pub slot_displays: IdMapping,
    pub data_component_type: IdMapping,
    pub menus: IdMapping,
    pub attributes: IdMapping,
    pub enchantments: IdMapping,
    pub paintings: IdMapping,
    pub tags: TagMappings,
}

impl StepMappings {
    fn identity() -> Self {
        Self {
            blockstates: IdMapping::IDENTITY,
            blocks: IdMapping::IDENTITY,
            items: IdMapping::IDENTITY,
            sounds: IdMapping::IDENTITY,
            blockentities: IdMapping::IDENTITY,
            entities: IdMapping::IDENTITY,
            particles: IdMapping::IDENTITY,
            argumenttypes: IdMapping::IDENTITY,
            statistics: IdMapping::IDENTITY,
            recipe_serializers: IdMapping::IDENTITY,
            slot_displays: IdMapping::IDENTITY,
            data_component_type: IdMapping::IDENTITY,
            menus: IdMapping::IDENTITY,
            attributes: IdMapping::IDENTITY,
            enchantments: IdMapping::IDENTITY,
            paintings: IdMapping::IDENTITY,
            tags: TagMappings::default(),
        }
    }

    fn load(step: &StepFile) -> Self {
        let Some(data) = step.data else {
            return Self::identity();
        };
        let root = read_root(data);
        let space = |key: &str| load_space(&root, key, step.from);
        Self {
            blockstates: space("blockstates"),
            blocks: space("blocks"),
            items: space("items"),
            sounds: space("sounds"),
            blockentities: space("blockentities"),
            entities: space("entities"),
            particles: space("particles"),
            argumenttypes: space("argumenttypes"),
            statistics: space("statistics"),
            recipe_serializers: space("recipe_serializers"),
            slot_displays: space("slot_displays"),
            data_component_type: space("data_component_type"),
            menus: space("menus"),
            attributes: space("attributes"),
            enchantments: space("enchantments"),
            paintings: space("paintings"),
            tags: load_tags(&root),
        }
    }
}

/// The composition of every step from 26.3 down to one target version.
pub struct ComposedMappings {
    pub blockstates: IdMapping,
    pub blocks: IdMapping,
    pub items: IdMapping,
    pub sounds: IdMapping,
    pub blockentities: IdMapping,
    pub entities: IdMapping,
    pub particles: IdMapping,
    pub argumenttypes: IdMapping,
    pub statistics: IdMapping,
    pub recipe_serializers: IdMapping,
    pub slot_displays: IdMapping,
    pub data_component_type: IdMapping,
    pub menus: IdMapping,
    pub attributes: IdMapping,
    pub enchantments: IdMapping,
    pub paintings: IdMapping,
    items_inverse: OnceLock<IdMapping>,
    data_component_type_inverse: OnceLock<IdMapping>,
}

impl ComposedMappings {
    fn identity() -> Self {
        Self {
            blockstates: IdMapping::IDENTITY,
            blocks: IdMapping::IDENTITY,
            items: IdMapping::IDENTITY,
            sounds: IdMapping::IDENTITY,
            blockentities: IdMapping::IDENTITY,
            entities: IdMapping::IDENTITY,
            particles: IdMapping::IDENTITY,
            argumenttypes: IdMapping::IDENTITY,
            statistics: IdMapping::IDENTITY,
            recipe_serializers: IdMapping::IDENTITY,
            slot_displays: IdMapping::IDENTITY,
            data_component_type: IdMapping::IDENTITY,
            menus: IdMapping::IDENTITY,
            attributes: IdMapping::IDENTITY,
            enchantments: IdMapping::IDENTITY,
            paintings: IdMapping::IDENTITY,
            items_inverse: OnceLock::new(),
            data_component_type_inverse: OnceLock::new(),
        }
    }

    fn compose(&self, step: &StepMappings) -> Self {
        Self {
            blockstates: self.blockstates.compose(&step.blockstates),
            blocks: self.blocks.compose(&step.blocks),
            items: self.items.compose(&step.items),
            sounds: self.sounds.compose(&step.sounds),
            blockentities: self.blockentities.compose(&step.blockentities),
            entities: self.entities.compose(&step.entities),
            particles: self.particles.compose(&step.particles),
            argumenttypes: self.argumenttypes.compose(&step.argumenttypes),
            statistics: self.statistics.compose(&step.statistics),
            recipe_serializers: self.recipe_serializers.compose(&step.recipe_serializers),
            slot_displays: self.slot_displays.compose(&step.slot_displays),
            data_component_type: self.data_component_type.compose(&step.data_component_type),
            menus: self.menus.compose(&step.menus),
            attributes: self.attributes.compose(&step.attributes),
            enchantments: self.enchantments.compose(&step.enchantments),
            paintings: self.paintings.compose(&step.paintings),
            items_inverse: OnceLock::new(),
            data_component_type_inverse: OnceLock::new(),
        }
    }

    /// Item ids of the target version mapped back onto 26.3.
    #[must_use]
    pub fn items_inverse(&self) -> &IdMapping {
        self.items_inverse.get_or_init(|| self.items.inverse())
    }

    /// Data component ids of the target version mapped back onto 26.3.
    #[must_use]
    pub fn data_component_type_inverse(&self) -> &IdMapping {
        self.data_component_type_inverse
            .get_or_init(|| self.data_component_type.inverse())
    }
}

pub struct MappingData {
    steps: Vec<OnceLock<StepMappings>>,
    composed: Vec<OnceLock<ComposedMappings>>,
    identity: StepMappings,
}

impl MappingData {
    #[must_use]
    pub fn get() -> &'static Self {
        static DATA: OnceLock<MappingData> = OnceLock::new();
        DATA.get_or_init(|| Self {
            steps: STEPS.iter().map(|_| OnceLock::new()).collect(),
            composed: (0..=STEPS.len()).map(|_| OnceLock::new()).collect(),
            identity: StepMappings::identity(),
        })
    }

    /// The tables of the step leaving `from`, identity for a version that is
    /// not a step start.
    #[must_use]
    pub fn step(&self, from: JavaMinecraftVersion) -> &StepMappings {
        let Some(index) = STEPS.iter().position(|step| step.from == from) else {
            return &self.identity;
        };
        self.steps[index].get_or_init(|| StepMappings::load(&STEPS[index]))
    }

    /// The 26.3 to `target` tables, built on first use.
    #[must_use]
    pub fn composed(&self, target: JavaMinecraftVersion) -> &ComposedMappings {
        let length = STEPS
            .iter()
            .filter(|step| step.to.protocol_version() >= target.protocol_version())
            .count();
        self.composed[length].get_or_init(|| {
            let mut composed = ComposedMappings::identity();
            for step in &STEPS[..length] {
                composed = composed.compose(self.step(step.from));
            }
            composed
        })
    }
}

/// The vendored files are plain named NBT; Via ships them gzipped.
fn read_root(data: &'static [u8]) -> NbtCompound {
    if data.starts_with(&[0x1f, 0x8b]) {
        return read_gzip_compound_tag(Cursor::new(data)).expect("gzip mapping file");
    }
    let mut reader = NbtReadHelperJava::new(Cursor::new(data));
    Nbt::read(&mut reader).expect("mapping file").root_tag
}

fn load_space(root: &NbtCompound, key: &str, from: JavaMinecraftVersion) -> IdMapping {
    if let Some(over) = OVERRIDES
        .iter()
        .find(|over| over.from == from && over.key == key)
    {
        return IdMapping(Repr::Table(over.table.to_vec()));
    }
    root.get_compound(key).map_or(IdMapping::IDENTITY, decode)
}

/// Decodes one id space with the strategy it was written with.
fn decode(tag: &NbtCompound) -> IdMapping {
    let strategy = tag.get_byte("id").expect("mapping strategy");
    let size = tag.get_int("size").expect("mapping size");
    let size = usize::try_from(size).expect("mapping size fits");
    match strategy {
        0 => {
            let mut values = Vec::with_capacity(size);
            let mut reader = ValReader::new(tag);
            let mut previous = 0;
            for _ in 0..size {
                previous += reader.zigzag_var_int();
                values.push(previous);
            }
            IdMapping(Repr::Table(values))
        }
        1 => {
            let pairs = ValReader::new(tag).at_value_pairs();
            let mut values = vec![-1; size];
            for (index, (at, to)) in pairs.iter().enumerate() {
                let end = pairs
                    .get(index + 1)
                    .map_or(size, |(next, _)| *next as usize);
                for (id, mapped) in (*at as usize..end).zip(*to..) {
                    values[id] = mapped;
                }
            }
            let first = pairs.first().map_or(size, |(at, _)| *at as usize);
            for (id, value) in values.iter_mut().enumerate().take(first) {
                *value = i32::try_from(id).expect("id fits");
            }
            IdMapping(Repr::Table(values))
        }
        2 => {
            let pairs = ValReader::new(tag).at_value_pairs();
            let fill = !tag.has("nofill");
            let mut values = vec![-1; size];
            let mut next_unhandled = 0;
            for (at, value) in &pairs {
                let at = *at as usize;
                if fill {
                    for (id, value) in values.iter_mut().enumerate().take(at).skip(next_unhandled) {
                        *value = i32::try_from(id).expect("id fits");
                    }
                    next_unhandled = at + 1;
                }
                values[at] = *value;
            }
            if fill {
                for (id, value) in values.iter_mut().enumerate().skip(next_unhandled) {
                    *value = i32::try_from(id).expect("id fits");
                }
            }
            IdMapping(Repr::Table(values))
        }
        3 => IdMapping(Repr::Bounded(size)),
        other => panic!("unknown mapping strategy {other}"),
    }
}

struct ValReader<'a> {
    data: &'a [i8],
    position: usize,
}

impl<'a> ValReader<'a> {
    fn new(tag: &'a NbtCompound) -> Self {
        Self {
            data: tag.get_byte_array("val").expect("mapping values"),
            position: 0,
        }
    }

    fn var_int(&mut self) -> i32 {
        let mut value = 0;
        let mut shift = 0;
        while self.position < self.data.len() {
            let byte = self.data[self.position] as u8;
            self.position += 1;
            value |= i32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        value
    }

    fn zigzag_var_int(&mut self) -> i32 {
        let value = self.var_int() as u32;
        (value >> 1) as i32 ^ -((value & 1) as i32)
    }

    /// Ids as the difference to the previous id, values as the zigzag
    /// difference to the previous value.
    fn at_value_pairs(mut self) -> Vec<(i32, i32)> {
        let mut pairs = Vec::new();
        let mut at = -1;
        let mut value = 0;
        while self.position < self.data.len() {
            at += 1 + self.var_int();
            value += self.zigzag_var_int();
            pairs.push((at, value));
        }
        pairs
    }
}

fn load_tags(root: &NbtCompound) -> TagMappings {
    let Some(tags) = root.get_compound("tags") else {
        return TagMappings::default();
    };
    let registries = tags
        .child_tags
        .iter()
        .filter_map(|(registry, tag)| {
            let NbtTag::Compound(tag) = tag else {
                return None;
            };
            let tags = tag
                .child_tags
                .iter()
                .map(|(name, ids)| (name.to_string(), decode_ids(ids)))
                .collect();
            Some((registry.to_string(), tags))
        })
        .collect();
    TagMappings { registries }
}

/// Tag members are either an int array or ranges packed into varints.
fn decode_ids(tag: &NbtTag) -> Vec<i32> {
    match tag {
        NbtTag::IntArray(ids) => ids.clone(),
        NbtTag::ByteArray(ranges) => {
            let mut reader = ValReader {
                data: ranges.as_ref(),
                position: 0,
            };
            let mut ids = Vec::new();
            let mut previous_end = 0;
            while reader.position < reader.data.len() {
                let start = previous_end + reader.var_int();
                let end = start + reader.var_int();
                ids.extend(start..=end);
                previous_end = end + 1;
            }
            ids
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ComposedMappings, IdMapping, MappingData};
    use crate::remap::{
        attribute_id_remap::remap_attribute_id_for_version,
        block_entity_type_id_remap::remap_block_entity_type_id_for_version,
        menu_id_remap::remap_menu_id_for_version, particle_id_remap::remap_particle_id_for_version,
        sound_id_remap::remap_sound_id_for_version,
    };
    use pumpkin_util::version::JavaMinecraftVersion;

    #[test]
    fn every_step_file_decodes() {
        for step in super::STEPS {
            let mappings = MappingData::get().step(step.from);
            if step.data.is_some() {
                assert!(!mappings.sounds.is_empty(), "{:?}", step.from);
            } else {
                assert!(mappings.sounds.is_identity() && mappings.items.is_identity());
            }
        }
    }

    #[test]
    fn composition_reaches_the_floor() {
        let composed = MappingData::get().composed(JavaMinecraftVersion::V_1_16_2);
        assert_eq!(composed.blockstates.len(), 35723);
        assert_eq!(composed.blockstates.map(0), Some(0));
        assert_eq!(composed.items.map(0), Some(0));
    }

    #[test]
    fn the_server_version_maps_nothing() {
        let composed = MappingData::get().composed(JavaMinecraftVersion::V_26_3);
        assert!(composed.items.is_identity());
        assert_eq!(composed.items.map(1657), Some(1657));
    }

    #[test]
    fn inverse_keeps_the_lowest_id() {
        let mapping = IdMapping(super::Repr::Table(vec![0, 1, 5, 3, 5]));
        assert_eq!(mapping.inverse().map(5), Some(2));
    }

    #[test]
    fn tags_dropped_after_the_step_are_listed() {
        let step = MappingData::get().step(JavaMinecraftVersion::V_1_21_5);
        let items = step.tags.get("item");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "dyeable");
        assert!(!items[0].1.is_empty());
    }

    fn write(hash: &mut u64, value: i64) {
        for byte in value.to_le_bytes() {
            *hash ^= u64::from(byte);
            *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn hash_mapping(hash: &mut u64, mapping: &IdMapping) {
        write(hash, mapping.len() as i64);
        for id in 0..mapping.len() as u32 {
            write(hash, mapping.map(id).map_or(-1, i64::from));
        }
    }

    fn hash_version(ids: &ComposedMappings) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325;
        for mapping in [
            &ids.blockstates,
            &ids.blocks,
            &ids.items,
            &ids.sounds,
            &ids.blockentities,
            &ids.entities,
            &ids.particles,
            &ids.argumenttypes,
            &ids.statistics,
            &ids.recipe_serializers,
            &ids.slot_displays,
            &ids.data_component_type,
            &ids.menus,
            &ids.attributes,
            &ids.enchantments,
            &ids.paintings,
            ids.items_inverse(),
            ids.data_component_type_inverse(),
        ] {
            hash_mapping(&mut hash, mapping);
        }
        hash
    }

    const SNAPSHOT: &[(JavaMinecraftVersion, u64)] = &[
        (JavaMinecraftVersion::V_1_16_2, 0x39f8_62d0_3e7e_04cd),
        (JavaMinecraftVersion::V_1_16_3, 0x39f8_62d0_3e7e_04cd),
        (JavaMinecraftVersion::V_1_16_4, 0x39f8_62d0_3e7e_04cd),
        (JavaMinecraftVersion::V_1_17, 0x4352_cebc_3dd4_343f),
        (JavaMinecraftVersion::V_1_17_1, 0x4352_cebc_3dd4_343f),
        (JavaMinecraftVersion::V_1_18, 0x1844_eb40_e1cd_1f0b),
        (JavaMinecraftVersion::V_1_18_2, 0x1844_eb40_e1cd_1f0b),
        (JavaMinecraftVersion::V_1_19, 0xa8ac_a754_e64c_e654),
        (JavaMinecraftVersion::V_1_19_1, 0xa8ac_a754_e64c_e654),
        (JavaMinecraftVersion::V_1_19_3, 0xd844_294b_09a3_25ae),
        (JavaMinecraftVersion::V_1_19_4, 0x7e04_eead_fd01_ecec),
        (JavaMinecraftVersion::V_1_20, 0x6381_ec85_7427_3399),
        (JavaMinecraftVersion::V_1_20_2, 0xbff1_f72a_c0a9_b66e),
        (JavaMinecraftVersion::V_1_20_3, 0x0a24_abd1_a9f4_0ce6),
        (JavaMinecraftVersion::V_1_20_5, 0xf350_9b82_b11d_10c9),
        (JavaMinecraftVersion::V_1_21, 0x529d_4d68_e307_facd),
        (JavaMinecraftVersion::V_1_21_2, 0x6dc1_5f60_0570_9b5b),
        (JavaMinecraftVersion::V_1_21_4, 0xe600_0cc3_2dc6_4cc6),
        (JavaMinecraftVersion::V_1_21_5, 0x82b2_5ecd_b689_cc43),
        (JavaMinecraftVersion::V_1_21_6, 0xeb56_d23b_06e1_97be),
        (JavaMinecraftVersion::V_1_21_7, 0xdfce_798e_de3c_1a2b),
        (JavaMinecraftVersion::V_1_21_9, 0xfa3f_823b_6026_98ee),
        (JavaMinecraftVersion::V_1_21_11, 0x2afd_edaf_46fc_8699),
        (JavaMinecraftVersion::V_26_1, 0x7aa5_e943_80b9_8267),
        (JavaMinecraftVersion::V_26_2, 0x8697_5743_4eca_daba),
    ];

    #[test]
    fn every_composed_table_matches_the_snapshot() {
        for (version, snapshot) in SNAPSHOT {
            let hash = hash_version(MappingData::get().composed(*version));
            assert_eq!(hash, *snapshot, "{version:?}");
        }
    }

    #[test]
    fn overridden_and_absent_ids() {
        // 1.19.4 split the smithing table; the mapping file carries no menus.
        assert_eq!(
            remap_menu_id_for_version(21, JavaMinecraftVersion::V_1_19_3),
            20
        );
        // `generic.max_absorption` arrived in 1.20.2.
        assert_eq!(
            remap_attribute_id_for_version(22, JavaMinecraftVersion::V_1_20),
            0
        );
        // `sculk_sensor` arrived in 1.17.
        assert_eq!(
            remap_block_entity_type_id_for_version(34, JavaMinecraftVersion::V_1_16_2),
            0
        );
        // An id the target lacks resolves to 0, not to whatever id 0 turns
        // into further down the chain.
        assert_eq!(
            remap_particle_id_for_version(8, JavaMinecraftVersion::V_1_20_3),
            0
        );
        assert_eq!(
            remap_sound_id_for_version(64, JavaMinecraftVersion::V_1_17),
            0
        );
    }
}
