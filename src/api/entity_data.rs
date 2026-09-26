//! The `SET_ENTITY_DATA` entry list, read and written in one layout.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::item::rewrite_item_value;
use crate::api::rewriter::particle::{PARTICLE, Particle, read_particle_for_layout};
use crate::api::types::{F32T, I64T, NbtT, STRING, VAR_INT, VAR_LONG, WireType};
use crate::data::entity_data_types::{
    MetaKind, canonical_meta_data_type_id_for_version, meta_kind,
};
use crate::data::mappings::{ComposedMappings, MappingData};

/// Ends the list; what vanilla writes after the last entry.
pub const TERMINATOR: u8 = 0xff;

/// One entry, with its 26.3 index and serializer id.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityDataEntry {
    pub index: u8,
    pub serializer: i32,
    pub value: MetaValue,
}

/// A decoded value. Registry ids are canonicalized to 26.3; `Raw` payloads
/// retain their layout-specific wire encoding.
#[derive(Clone, Debug, PartialEq)]
pub enum MetaValue {
    Raw(Vec<u8>),
    BlockState(i32),
    OptionalBlockState(i32),
    Particle(Particle),
    Particles(Vec<Particle>),
    PaintingVariant(i32),
    Item(Vec<u8>),
}

/// Reads and writes a metadata list in `layout`; a value it cannot measure
/// ends the list.
#[derive(Clone, Copy)]
pub struct EntityDataListT {
    layout: JavaMinecraftVersion,
    ids: &'static ComposedMappings,
}

impl EntityDataListT {
    #[must_use]
    pub fn for_version(layout: JavaMinecraftVersion) -> Self {
        Self {
            layout,
            ids: MappingData::get().composed(layout),
        }
    }
}

impl WireType for EntityDataListT {
    type Value = Vec<EntityDataEntry>;

    fn read(&self, r: &mut &[u8]) -> Result<Self::Value, ReadingError> {
        let mut entries = Vec::new();
        loop {
            let Some((&index, rest)) = r.split_first() else {
                return Err(ReadingError::Incomplete("entity data".into()));
            };
            if index == TERMINATOR {
                *r = rest;
                return Ok(entries);
            }
            let mut cursor = rest;
            let Some(entry) = read_entry(index, &mut cursor, self.layout, self.ids) else {
                // Nothing after an entry we cannot measure can be found again.
                *r = &[];
                return Ok(entries);
            };
            *r = cursor;
            entries.push(entry);
        }
    }

    fn write(&self, w: &mut Vec<u8>, v: &Self::Value) -> Result<(), WritingError> {
        for entry in v {
            w.write_u8(entry.index)?;
            w.write_var_int(&VarInt(entry.serializer))?;
            write_value(&entry.value, w)?;
        }
        w.write_u8(TERMINATOR)
    }
}

fn read_entry(
    index: u8,
    r: &mut &[u8],
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Option<EntityDataEntry> {
    let wire_serializer = r.get_var_int().ok()?.0;
    let serializer = canonical_meta_data_type_id_for_version(wire_serializer, layout)?;
    let value = read_value(meta_kind(serializer)?, r, layout, ids)?;
    Some(EntityDataEntry {
        index,
        serializer,
        value,
    })
}

/// The bytes `t` consumed, which is what a value with no id inside is written back as.
fn raw<T: WireType>(t: &T, r: &mut &[u8]) -> Option<Vec<u8>> {
    let before = *r;
    t.read(r).ok()?;
    Some(before[..before.len() - r.len()].to_vec())
}

fn take(r: &mut &[u8], count: usize) -> Option<Vec<u8>> {
    if r.len() < count {
        return None;
    }
    let (head, rest) = r.split_at(count);
    *r = rest;
    Some(head.to_vec())
}

fn read_value(
    kind: MetaKind,
    r: &mut &[u8],
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Option<MetaValue> {
    let raw = match kind {
        MetaKind::Byte | MetaKind::Bool => take(r, 1)?,
        MetaKind::Float => take(r, 4)?,
        MetaKind::Rotations | MetaKind::Vector3 => take(r, 12)?,
        MetaKind::BlockPos => take(r, 8)?,
        MetaKind::Quaternion => take(r, 16)?,
        MetaKind::VarInt => raw(&VAR_INT, r)?,
        MetaKind::VarLong => raw(&VAR_LONG, r)?,
        MetaKind::String => raw(&STRING, r)?,
        MetaKind::Component => raw_component(r, layout)?,
        MetaKind::OptionalComponent => {
            let mut out = take(r, 1)?;
            if out[0] != 0 {
                out.extend(raw_component(r, layout)?);
            }
            out
        }
        MetaKind::OptionalBlockPos => optional(r, 8)?,
        MetaKind::OptionalUuid => optional(r, 16)?,
        MetaKind::VillagerData => {
            let mut out = raw(&VAR_INT, r)?;
            out.extend(raw(&VAR_INT, r)?);
            out.extend(raw(&VAR_INT, r)?);
            out
        }
        MetaKind::BlockState => {
            let wire_state = u32::try_from(r.get_var_int().ok()?.0).ok()?;
            let canonical_state = ids.blockstates_inverse().map(wire_state)?;
            return Some(MetaValue::BlockState(i32::try_from(canonical_state).ok()?));
        }
        MetaKind::OptionalBlockState => {
            let wire_state = r.get_var_int().ok()?.0;
            if wire_state == 0 {
                return Some(MetaValue::OptionalBlockState(0));
            }
            let canonical_state = ids
                .blockstates_inverse()
                .map(u32::try_from(wire_state).ok()?)?;
            return Some(MetaValue::OptionalBlockState(
                i32::try_from(canonical_state).ok()?,
            ));
        }
        MetaKind::PaintingVariant => {
            let wire_variant = r.get_var_int().ok()?.0;
            if wire_variant == 0 {
                return Some(MetaValue::PaintingVariant(0));
            }
            let wire_variant = wire_variant.checked_sub(1)?;
            let canonical_variant = ids
                .paintings_inverse()
                .map(u32::try_from(wire_variant).ok()?)?;
            return Some(MetaValue::PaintingVariant(
                i32::try_from(canonical_variant).ok()?.checked_add(1)?,
            ));
        }
        MetaKind::Particle => {
            return Some(MetaValue::Particle(
                read_particle_for_layout(r, layout, ids).ok()?,
            ));
        }
        MetaKind::Particles => {
            let count = r.get_var_int().ok()?.0;
            if !(0..=4096).contains(&count) {
                return None;
            }
            let mut particles = Vec::new();
            for _ in 0..count {
                particles.push(read_particle_for_layout(r, layout, ids).ok()?);
            }
            return Some(MetaValue::Particles(particles));
        }
        MetaKind::Item => return Some(MetaValue::Item(rewrite_item_value(r, layout, ids)?)),
    };
    Some(MetaValue::Raw(raw))
}

fn optional(r: &mut &[u8], count: usize) -> Option<Vec<u8>> {
    let mut out = take(r, 1)?;
    if out[0] != 0 {
        out.extend(take(r, count)?);
    }
    Some(out)
}

/// A json string below 1.20.3, network nbt from it.
fn raw_component(r: &mut &[u8], layout: JavaMinecraftVersion) -> Option<Vec<u8>> {
    if layout < JavaMinecraftVersion::V_1_20_3 {
        raw(&STRING, r)
    } else {
        raw(&NbtT::for_version(layout), r)
    }
}

/// The layout does not change, so a value with no id inside goes back verbatim.
fn write_value(value: &MetaValue, w: &mut Vec<u8>) -> Result<(), WritingError> {
    match value {
        MetaValue::Raw(bytes) | MetaValue::Item(bytes) => w.write_slice(bytes),
        MetaValue::BlockState(id)
        | MetaValue::OptionalBlockState(id)
        | MetaValue::PaintingVariant(id) => w.write_var_int(&VarInt(*id)),
        MetaValue::Particle(particle) => PARTICLE.write(w, particle),
        MetaValue::Particles(particles) => {
            w.write_var_int(&VarInt(
                i32::try_from(particles.len())
                    .map_err(|_| WritingError::Message("too many particles".to_string()))?,
            ))?;
            for particle in particles {
                PARTICLE.write(w, particle)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::rewriter::particle::ParticleData;
    use pumpkin_data::particle::Particle as ParticleKind;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    /// A pig's shared flags, health and absent custom name, in canonical
    /// 26.3 serializer numbering.
    fn pig_entries() -> Vec<u8> {
        let mut out = vec![0, 0, 0x08];
        out.extend([9, 3]);
        out.extend(10.0f32.to_be_bytes());
        out.extend([2, 6, 0]);
        out.push(TERMINATOR);
        out
    }

    #[test]
    fn a_canonical_list_round_trips_in_the_26_3_layout() {
        let payload = pig_entries();
        let list = EntityDataListT::for_version(V::V_26_3);
        let mut read: &[u8] = &payload;
        let entries = list.read(&mut read).unwrap();
        assert!(read.is_empty());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].index, 9);
        assert_eq!(entries[1].serializer, 3);
        let mut out = Vec::new();
        list.write(&mut out, &entries).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn a_legacy_rotation_serializer_is_not_misread_as_the_canonical_boolean() {
        for layout in [V::V_1_15_2, V::V_1_16, V::V_1_16_1, V::V_1_16_2] {
            let mut payload = vec![9, 8]; // index 9, wire serializer 8 (rotation)
            payload.extend([0x3f, 0x80, 0, 0]);
            payload.extend([0u8; 8]);
            payload.push(TERMINATOR);

            let mut read: &[u8] = &payload;
            let entries = EntityDataListT::for_version(layout)
                .read(&mut read)
                .unwrap();
            assert!(read.is_empty(), "{layout}");
            assert_eq!(entries.len(), 1, "{layout}");
            assert_eq!(entries[0].serializer, 9, "canonical rotations id, {layout}");
            assert_eq!(entries[0].value, MetaValue::Raw(payload[2..14].to_vec()));
        }
    }

    #[test]
    fn a_truncated_legacy_rotation_does_not_become_a_boolean_entry() {
        let mut payload = vec![9, 8];
        payload.extend([0u8; 11]);
        payload.push(TERMINATOR);
        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(V::V_1_15_2)
            .read(&mut read)
            .unwrap();
        assert!(entries.is_empty());
        assert!(
            read.is_empty(),
            "unmeasurable metadata is discarded as a suffix"
        );
    }

    #[test]
    fn a_legacy_block_state_is_normalized_before_the_global_forward_map() {
        let layout = V::V_1_16_2;
        let ids = MappingData::get().composed(layout);
        let canonical_state = (0..u32::try_from(ids.blockstates.len()).unwrap())
            .find(|state| {
                ids.blockstates.map(*state).is_some_and(|wire| {
                    wire != *state && ids.blockstates_inverse().map(wire) == Some(*state)
                })
            })
            .expect("a block state whose id changes on 1.16.2");
        let wire_state = ids.blockstates.map(canonical_state).unwrap();
        let mut payload = vec![8u8, 13]; // 1.16 optional block-state serializer
        VAR_INT
            .write(&mut payload, &VarInt(i32::try_from(wire_state).unwrap()))
            .unwrap();
        payload.push(TERMINATOR);

        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(layout)
            .read(&mut read)
            .unwrap();
        assert_eq!(
            entries[0].serializer, 15,
            "canonical optional-blockstate id"
        );
        assert_eq!(
            entries[0].value,
            MetaValue::OptionalBlockState(i32::try_from(canonical_state).unwrap())
        );
        assert!(read.is_empty());
    }

    #[test]
    fn a_1_16_float_dust_shape_and_particle_id_become_canonical() {
        let layout = V::V_1_16;
        let ids = MappingData::get().composed(layout);
        let dust = i32::from(ParticleKind::Dust.to_id());
        let wire_dust = ids.particles.map(u32::try_from(dust).unwrap()).unwrap();
        let mut payload = vec![10u8, 15]; // legacy particle serializer
        VAR_INT
            .write(&mut payload, &VarInt(i32::try_from(wire_dust).unwrap()))
            .unwrap();
        for color in [1.0f32, 0.0, 0.5, 1.0] {
            F32T.write(&mut payload, &color).unwrap();
        }
        payload.push(TERMINATOR);

        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(layout)
            .read(&mut read)
            .unwrap();
        assert_eq!(
            entries[0].serializer, 16,
            "canonical particle serializer id"
        );
        assert_eq!(
            entries[0].value,
            MetaValue::Particle(Particle {
                id: dust,
                data: ParticleData::Dust {
                    rgb: 0xff0080,
                    scale: 1.0,
                },
            })
        );
        assert!(read.is_empty());
    }

    #[test]
    fn a_component_is_a_json_string_below_1_20_3_and_nbt_from_it() {
        let mut json = vec![2u8, 5];
        json.extend(b"\x02hi");
        json.push(TERMINATOR);
        let mut read: &[u8] = &json;
        let entries = EntityDataListT::for_version(V::V_1_20_2)
            .read(&mut read)
            .unwrap();
        assert_eq!(entries[0].value, MetaValue::Raw(b"\x02hi".to_vec()));

        // A single nbt string tag, which is what 1.20.3 and up read.
        let nbt = vec![2u8, 5, 0x08, 0x00, 0x02, b'h', b'i', TERMINATOR];
        let mut read: &[u8] = &nbt;
        let entries = EntityDataListT::for_version(V::V_1_20_3)
            .read(&mut read)
            .unwrap();
        assert_eq!(
            entries[0].value,
            MetaValue::Raw(vec![0x08, 0x00, 0x02, b'h', b'i'])
        );
        assert!(read.is_empty());
    }

    #[test]
    fn a_block_state_and_an_optional_one_come_out_as_ids() {
        let payload = vec![9u8, 14, 0x80, 0x02, 11, 15, 0, TERMINATOR];
        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(V::V_1_21_4)
            .read(&mut read)
            .unwrap();
        assert_eq!(entries[0].value, MetaValue::BlockState(256));
        assert_eq!(entries[1].value, MetaValue::OptionalBlockState(0));
    }

    fn effect_particle() -> u8 {
        u8::try_from(ParticleKind::EntityEffect.to_id()).expect("a one byte particle id")
    }

    #[test]
    fn an_effect_particle_list_keeps_its_colour() {
        let mut payload = vec![10u8, 17, 1, effect_particle()];
        payload.extend([0x11, 0x22, 0x33, 0x44, TERMINATOR]);
        let list = EntityDataListT::for_version(V::V_26_2);
        let mut read: &[u8] = &payload;
        let entries = list.read(&mut read).unwrap();
        let MetaValue::Particles(particles) = &entries[0].value else {
            panic!("not a particle list");
        };
        assert_eq!(particles.len(), 1);
        assert_eq!(
            particles[0].id,
            i32::from(ParticleKind::EntityEffect.to_id())
        );
        assert_eq!(particles[0].data, ParticleData::Color(0x1122_3344));
        let mut out = Vec::new();
        list.write(&mut out, &entries).unwrap();
        assert_eq!(out, payload);
    }

    /// The 26.2 particle serializer and particle id still match the canonical
    /// layout used by `read_particle`.
    #[test]
    fn an_effect_particle_is_read_in_the_canonical_layout_on_26_2() {
        let mut payload = vec![10u8, 16, effect_particle()];
        payload.extend([0x11, 0x22, 0x33, 0x44, 8, 0, 1, TERMINATOR]);
        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(V::V_26_2)
            .read(&mut read)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].value,
            MetaValue::Particle(Particle {
                id: i32::from(ParticleKind::EntityEffect.to_id()),
                data: ParticleData::Color(0x1122_3344),
            })
        );
        assert!(read.is_empty());
    }

    /// An empty stack is a single zero varint in the 26.3 form and stays one.
    #[test]
    fn an_item_entry_is_read_through_the_item_rewriter() {
        let payload = vec![0u8, 0, 0x08, 8, 7, 0, 9, 3, 0, 0, 0, 0, TERMINATOR];
        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(V::V_1_21_4)
            .read(&mut read)
            .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].index, 8);
        assert_eq!(entries[1].value, MetaValue::Item(vec![0]));
        assert!(read.is_empty());
    }

    #[test]
    fn a_serializer_with_no_reader_ends_the_list() {
        // 41 is resolvable_profile, which nothing writes.
        let payload = vec![0u8, 0, 0x08, 8, 41, 1, 2, 3, TERMINATOR];
        let mut read: &[u8] = &payload;
        let entries = EntityDataListT::for_version(V::V_26_2)
            .read(&mut read)
            .unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn a_list_without_a_terminator_is_an_error() {
        let mut read: &[u8] = &[0u8, 0, 0x08];
        assert!(
            EntityDataListT::for_version(V::V_26_2)
                .read(&mut read)
                .is_err()
        );
    }
}
