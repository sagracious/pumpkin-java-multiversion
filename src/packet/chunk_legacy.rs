//! Turns a `LEVEL_CHUNK_WITH_LIGHT` payload back into the shapes 1.16.2 to
//! 1.17.1 read.
//!
//! Core only ever writes the 1.18 form, so below that we reframe it into a primary bit mask, flat biome array, blocks-only sections and full NBT block entities, cutting sections outside the 0-255 world.

use std::io::Cursor;

use pumpkin_nbt::Nbt;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::{IdMapping, MappingData};

/// Blocks in a section.
const BLOCKS_PER_SECTION: usize = 16 * 16 * 16;
/// Biomes in a 1.18 section, and a quarter of the column's own 4x4x4 grid.
const BIOMES_PER_SECTION: usize = 4 * 4 * 4;
/// Biomes in a 0 to 255 column.
const BIOMES_PER_COLUMN: usize = 1024;
/// Sections in a 0 to 255 world.
const SECTIONS: usize = 16;
/// Narrowest indirect block palette vanilla reads below 1.18.
const LEGACY_BLOCK_BITS: u8 = 4;
/// Widest indirect block palette, above which the section is direct.
const MAX_INDIRECT_BLOCK_BITS: u8 = 8;
/// Same threshold for the biome container.
const MAX_INDIRECT_BIOME_BITS: u8 = 3;

/// Entries that fit in one packed long at `bits`, which never span a boundary
/// from 1.16 on.
const fn per_long(bits: u8) -> usize {
    64 / bits as usize
}

fn unpack(longs: &[i64], bits: u8, count: usize) -> Option<Vec<u32>> {
    let per = per_long(bits);
    if longs.len() < count.div_ceil(per) {
        return None;
    }
    let mask = (1u64 << bits) - 1;
    Some(
        (0..count)
            .map(|index| {
                let word = longs[index / per] as u64;
                ((word >> (bits as usize * (index % per))) & mask) as u32
            })
            .collect(),
    )
}

fn read_longs(cursor: &mut &[u8]) -> Option<Vec<i64>> {
    let count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    let mut longs = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        longs.push(cursor.get_i64_be().ok()?);
    }
    Some(longs)
}

fn write_longs(out: &mut Vec<u8>, longs: &[i64]) -> Option<()> {
    out.write_var_int(&VarInt(i32::try_from(longs.len()).ok()?))
        .ok()?;
    for &packed in longs {
        out.write_i64_be(packed).ok()?;
    }
    Some(())
}

fn read_palette(cursor: &mut &[u8]) -> Option<Vec<u32>> {
    let len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    let mut palette = Vec::with_capacity(len.min(4096));
    for _ in 0..len {
        palette.push(u32::try_from(cursor.get_var_int().ok()?.0).ok()?);
    }
    Some(palette)
}

/// One section's block container, already in the shape 1.16 and 1.17 read.
struct BlockContainer {
    bits: u8,
    palette: Vec<u32>,
    packed: Vec<i64>,
}

impl BlockContainer {
    /// The single value palette arrived with 1.18; below it vanilla rounds any
    /// width up to four and reads a palette list, so the one id becomes a one
    /// entry palette every block indexes.
    fn read(cursor: &mut &[u8]) -> Option<Self> {
        let bits = cursor.get_u8().ok()?;
        if bits == 0 {
            let id = u32::try_from(cursor.get_var_int().ok()?.0).ok()?;
            read_longs(cursor)?;
            return Some(Self {
                bits: LEGACY_BLOCK_BITS,
                palette: vec![id],
                packed: vec![0; BLOCKS_PER_SECTION / per_long(LEGACY_BLOCK_BITS)],
            });
        }
        if bits > MAX_INDIRECT_BLOCK_BITS {
            // A direct palette stores ids at a width taken from the sender's
            // registry size, so they cannot be renumbered where they lie.
            return None;
        }
        let palette = read_palette(cursor)?;
        let packed = read_longs(cursor)?;
        Some(Self {
            bits,
            palette,
            packed,
        })
    }

    fn write(&self, out: &mut Vec<u8>, states: &IdMapping) -> Option<()> {
        out.write_u8(self.bits).ok()?;
        out.write_var_int(&VarInt(i32::try_from(self.palette.len()).ok()?))
            .ok()?;
        for &state in &self.palette {
            let mapped = states.map(state).unwrap_or(0);
            out.write_var_int(&VarInt(i32::try_from(mapped).ok()?))
                .ok()?;
        }
        write_longs(out, &self.packed)
    }
}

/// Unpacks one section's biomes, which the column carries as one flat array
/// below 1.18.
fn read_biomes(cursor: &mut &[u8]) -> Option<Vec<u32>> {
    let bits = cursor.get_u8().ok()?;
    if bits == 0 {
        let id = u32::try_from(cursor.get_var_int().ok()?.0).ok()?;
        read_longs(cursor)?;
        return Some(vec![id; BIOMES_PER_SECTION]);
    }
    let palette = if bits <= MAX_INDIRECT_BIOME_BITS {
        Some(read_palette(cursor)?)
    } else {
        None
    };
    let packed = read_longs(cursor)?;
    let entries = unpack(&packed, bits, BIOMES_PER_SECTION)?;
    match palette {
        Some(palette) => entries
            .iter()
            .map(|&index| palette.get(index as usize).copied())
            .collect(),
        None => Some(entries),
    }
}

/// One tag with a root name, which is how every NBT below 1.20.2 travels.
fn read_named_compound(cursor: &mut &[u8]) -> Option<NbtCompound> {
    let mut nbt_cursor = Cursor::new(*cursor);
    let nbt = Nbt::read(&mut NbtReadHelperJava::new(&mut nbt_cursor)).ok()?;
    let used = usize::try_from(nbt_cursor.position()).ok()?;
    *cursor = cursor.get(used..)?;
    Some(nbt.root_tag)
}

/// Copies the heightmaps, which are a named root compound on every version
/// this covers.
fn copy_named_nbt(cursor: &mut &[u8], out: &mut Vec<u8>) -> Option<()> {
    let start = *cursor;
    let tag_id = cursor.get_u8().ok()?;
    if tag_id != 0 {
        let name_len = usize::try_from(cursor.get_i16_be().ok()?).ok()?;
        *cursor = cursor.get(name_len..)?;
        let body = *cursor;
        let mut nbt_cursor = Cursor::new(body);
        let mut reader = NbtReadHelperJava::new(&mut nbt_cursor);
        NbtTag::skip_data(&mut reader, tag_id).ok()?;
        let used = usize::try_from(nbt_cursor.position()).ok()?;
        // `skip_content` ends a compound on EOF as well as on TAG_End, so a
        // payload cut off inside one would skip clean.
        if tag_id == 10 && body.get(used.checked_sub(1)?) != Some(&0) {
            return None;
        }
        *cursor = &cursor[used..];
    }
    let consumed = start.len() - cursor.len();
    out.extend_from_slice(&start[..consumed]);
    Some(())
}

/// The 26.3 block entity name a `layout` numbered id stands for. Below 1.18
/// the client is given the name rather than the id, so the composed table the
/// id pass applied has to be read backwards.
fn block_entity_name(id: i32, inverse: &IdMapping) -> Option<&'static str> {
    let native = usize::try_from(inverse.map(u32::try_from(id).ok()?)?).ok()?;
    pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
        .get(native)
        .copied()
}

/// Rewrites a chunk from the 1.18 layout into the 1.17.1 one.
///
/// `world_bottom` is the y the server's own sections start at, which is what
/// decides where the client's 0 to 255 window sits in them.
#[must_use]
pub fn to_v1_17(
    payload: &[u8],
    world_bottom: i32,
    states: &IdMapping,
    layout: JavaMinecraftVersion,
) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;

    let mut heightmaps = Vec::new();
    copy_named_nbt(&mut cursor, &mut heightmaps)?;

    let blob_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if blob_len > cursor.len() {
        return None;
    }
    let (blob, rest) = cursor.split_at(blob_len);

    let first = usize::try_from((-world_bottom).max(0) / 16).ok()?;
    let mut sections = blob;
    let mut index = 0usize;
    let mut mask = 0u64;
    let mut kept = Vec::new();
    let mut biomes = Vec::with_capacity(BIOMES_PER_COLUMN);
    while !sections.is_empty() {
        let block_count = sections.get_i16_be().ok()?;
        let blocks = BlockContainer::read(&mut sections)?;
        let section_biomes = read_biomes(&mut sections)?;

        if let Some(slot) = index.checked_sub(first)
            && slot < SECTIONS
        {
            if block_count != 0 {
                mask |= 1 << slot;
                kept.write_i16_be(block_count).ok()?;
                blocks.write(&mut kept, states)?;
            }
            biomes.extend_from_slice(&section_biomes);
        }
        index += 1;
    }
    // A world shorter than the client's own: the rest of the column is the
    // biome the top section carried.
    let pad = biomes.last().copied().unwrap_or(0);
    biomes.resize(BIOMES_PER_COLUMN, pad);

    let mut out = Vec::with_capacity(payload.len());
    out.write_i32_be(chunk_x).ok()?;
    out.write_i32_be(chunk_z).ok()?;
    write_longs(&mut out, &[mask as i64])?;
    out.extend_from_slice(&heightmaps);
    out.write_var_int(&VarInt(i32::try_from(biomes.len()).ok()?))
        .ok()?;
    for biome in &biomes {
        out.write_var_int(&VarInt(i32::try_from(*biome).ok()?))
            .ok()?;
    }
    out.write_var_int(&VarInt(i32::try_from(kept.len()).ok()?))
        .ok()?;
    out.extend_from_slice(&kept);

    let mut cursor = rest;
    write_block_entities(&mut cursor, chunk_x, chunk_z, layout, &mut out)?;
    // The light rides in its own packet below 1.18, so it is only read to make
    // sure this really was the layout it was taken for.
    skip_light(&mut cursor)?;
    cursor.is_empty().then_some(out)
}

/// Walks the light block a 1.18 chunk ends with: a trust edges flag, four
/// masks and the two arrays they name.
fn skip_light(cursor: &mut &[u8]) -> Option<()> {
    cursor.get_bool().ok()?;
    for _ in 0..4 {
        read_longs(cursor)?;
    }
    for _ in 0..2 {
        let count = cursor.get_var_int().ok()?.0;
        for _ in 0..count {
            let len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
            *cursor = cursor.get(len..)?;
        }
    }
    Some(())
}

/// Turns the compact block entity list into the full NBT one, naming each
/// type and giving it back its position. The light data behind it is dropped:
/// below 1.18 it rides in its own packet.
fn write_block_entities(
    cursor: &mut &[u8],
    chunk_x: i32,
    chunk_z: i32,
    layout: JavaMinecraftVersion,
    out: &mut Vec<u8>,
) -> Option<()> {
    let count = cursor.get_var_int().ok()?.0;
    let inverse = MappingData::get().composed(layout).blockentities.inverse();

    let mut kept = 0i32;
    let mut entries = Vec::new();
    for _ in 0..count {
        let packed_xz = cursor.get_u8().ok()?;
        let y = cursor.get_i16_be().ok()?;
        let id = cursor.get_var_int().ok()?.0;
        let mut compound = read_named_compound(cursor)?;

        let Some(name) = block_entity_name(id, &inverse) else {
            continue;
        };
        compound.put_string("id", format!("minecraft:{name}"));
        compound.put_int("x", chunk_x * 16 + i32::from(packed_xz >> 4));
        compound.put_int("y", i32::from(y));
        compound.put_int("z", chunk_z * 16 + i32::from(packed_xz & 0xF));
        entries.extend_from_slice(&Nbt::new(String::new(), compound).write());
        kept += 1;
    }

    out.write_var_int(&VarInt(kept)).ok()?;
    out.extend_from_slice(&entries);
    Some(())
}

/// Rewrites a chunk from the 1.17 layout into the 1.16.4 one, where the mask
/// is a varint behind a "full chunk" flag, and renumbers the palettes on the
/// way.
#[must_use]
pub fn to_v1_16(payload: &[u8], states: &IdMapping) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;
    let words = read_longs(&mut cursor)?;
    let mask = match words.as_slice() {
        [] => 0,
        [word] => *word,
        // More than 64 sections cannot be named by a varint mask.
        _ => return None,
    };

    let mut out = Vec::with_capacity(payload.len());
    out.write_i32_be(chunk_x).ok()?;
    out.write_i32_be(chunk_z).ok()?;
    out.write_bool(true).ok()?;
    out.write_var_int(&VarInt(i32::try_from(mask).ok()?)).ok()?;
    copy_rest_of_column(&mut cursor, &mut out, mask, states)?;
    Some(out)
}

/// Everything a 1.16 and a 1.17 chunk share once the mask is behind them.
fn copy_rest_of_column(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    mask: i64,
    states: &IdMapping,
) -> Option<()> {
    copy_named_nbt(cursor, out)?;

    let biome_count = cursor.get_var_int().ok()?.0;
    out.write_var_int(&VarInt(biome_count)).ok()?;
    for _ in 0..biome_count {
        let biome = cursor.get_var_int().ok()?.0;
        out.write_var_int(&VarInt(biome)).ok()?;
    }

    let blob_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if blob_len > cursor.len() {
        return None;
    }
    let (blob, rest) = cursor.split_at(blob_len);

    let mut sections = blob;
    let mut rewritten = Vec::with_capacity(blob.len());
    for _ in 0..(mask as u64).count_ones() {
        let block_count = sections.get_i16_be().ok()?;
        rewritten.write_i16_be(block_count).ok()?;
        BlockContainer::read(&mut sections)?.write(&mut rewritten, states)?;
    }
    if !sections.is_empty() {
        return None;
    }

    out.write_var_int(&VarInt(i32::try_from(rewritten.len()).ok()?))
        .ok()?;
    out.extend_from_slice(&rewritten);
    out.extend_from_slice(rest);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{MappingData, remove_connection, with_connection};
    use crate::packet::mappings::clientbound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::java::client::play::{
        CChunkData, ChunkBlockEntity, ChunkHeightmaps, LightData,
    };

    const LAYOUT: JavaMinecraftVersion = JavaMinecraftVersion::V_1_18;
    /// The overworld the server has: y -64 to 319, twenty four sections.
    const WORLD_BOTTOM: i32 = -64;
    /// Section four of those is the client's own bottom one.
    const SOLID: usize = 4;

    /// The section blob core writes: every section a single value palette,
    /// section [`SOLID`] solid stone and the rest air, each with its own
    /// single value biome so the window can be checked.
    fn blob(count: usize, stone: i32) -> Vec<u8> {
        let mut blob = Vec::new();
        for index in 0..count {
            let solid = index == SOLID;
            blob.write_i16_be(if solid { 4096 } else { 0 }).unwrap();
            blob.write_u8(0).unwrap();
            blob.write_var_int(&VarInt(if solid { stone } else { 0 }))
                .unwrap();
            blob.write_var_int(&VarInt(0)).unwrap();
            blob.write_u8(0).unwrap();
            blob.write_var_int(&VarInt(i32::try_from(index).unwrap()))
                .unwrap();
            blob.write_var_int(&VarInt(0)).unwrap();
        }
        blob
    }

    /// One chunk packet through upstream's own writer, in the layout core
    /// writes on every version.
    fn chunk_packet(version: JavaMinecraftVersion, data: &[u8], block_entity: i32) -> Vec<u8> {
        let mut chest = NbtCompound::new();
        chest.put_string("CustomName", "crate".to_string());
        let packet = CChunkData::new(
            3,
            -2,
            ChunkHeightmaps::default(),
            data,
            vec![ChunkBlockEntity::new(0x21, 70, VarInt(block_entity), chest)],
            LightData::default(),
        );
        let mut payload = Vec::new();
        packet.write_packet_data(&mut payload, &version).unwrap();
        payload
    }

    fn native_chest() -> u32 {
        u32::try_from(
            pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
                .iter()
                .position(|&name| name == "chest")
                .unwrap(),
        )
        .unwrap()
    }

    fn stone() -> i32 {
        i32::from(pumpkin_data::Block::STONE.default_state.id.as_u16())
    }

    /// The packet as the id pass leaves it: ids already in 1.18 numbering.
    fn after_id_pass(data: &[u8]) -> Vec<u8> {
        let composed = MappingData::get().composed(LAYOUT);
        chunk_packet(
            LAYOUT,
            data,
            i32::try_from(composed.blockentities.map(native_chest()).unwrap()).unwrap(),
        )
    }

    fn to_1_17(data: &[u8]) -> Vec<u8> {
        to_v1_17(
            &after_id_pass(data),
            WORLD_BOTTOM,
            &MappingData::get().step(LAYOUT).blockstates,
            LAYOUT,
        )
        .expect("converted")
    }

    /// The 1.17 container is x, z, the `BitSet` mask, the heightmaps, 1024
    /// biomes, the section blob and the full NBT block entities, which is
    /// minecraft-data's `packet_map_chunk` for `pc/1.17.1`.
    #[test]
    fn a_1_18_chunk_becomes_a_1_17_one() {
        let out = to_1_17(&blob(24, stone()));
        let states = &MappingData::get().step(LAYOUT).blockstates;

        let mut read: &[u8] = &out;
        assert_eq!(read.get_i32_be().unwrap(), 3);
        assert_eq!(read.get_i32_be().unwrap(), -2);
        assert_eq!(read_longs(&mut read).unwrap(), vec![1], "section 0 only");
        let mut heightmaps = Vec::new();
        copy_named_nbt(&mut read, &mut heightmaps).unwrap();

        let biome_count = read.get_var_int().unwrap().0;
        assert_eq!(biome_count, i32::try_from(BIOMES_PER_COLUMN).unwrap());
        let biomes: Vec<i32> = (0..biome_count)
            .map(|_| read.get_var_int().unwrap().0)
            .collect();
        // The window is the server's sections four to nineteen, y 0 to 255.
        assert_eq!(biomes[0], i32::try_from(SOLID).unwrap());
        assert_eq!(
            biomes[BIOMES_PER_SECTION],
            i32::try_from(SOLID).unwrap() + 1
        );
        assert_eq!(biomes[BIOMES_PER_COLUMN - 1], 19);

        let blob_len = usize::try_from(read.get_var_int().unwrap().0).unwrap();
        let (mut sections, mut rest) = read.split_at(blob_len);
        assert_eq!(sections.get_i16_be().unwrap(), 4096);
        assert_eq!(
            sections.get_u8().unwrap(),
            LEGACY_BLOCK_BITS,
            "1.17 has no single value palette"
        );
        assert_eq!(sections.get_var_int().unwrap().0, 1);
        assert_eq!(
            u32::try_from(sections.get_var_int().unwrap().0).unwrap(),
            states.map(u32::try_from(stone()).unwrap()).unwrap()
        );
        assert_eq!(read_longs(&mut sections).unwrap().len(), 256);
        assert!(sections.is_empty(), "only the one non empty section");

        assert_eq!(rest.get_var_int().unwrap().0, 1);
        let entity = read_named_compound(&mut rest).unwrap();
        assert_eq!(entity.get_string("id"), Some("minecraft:chest"));
        assert_eq!(entity.get_string("CustomName"), Some("crate"));
        assert_eq!(entity.get_int("x"), Some(3 * 16 + 2));
        assert_eq!(entity.get_int("y"), Some(70));
        assert_eq!(entity.get_int("z"), Some(-2 * 16 + 1));
        assert!(rest.is_empty(), "the light rides in its own packet");
    }

    /// 1.16.2 reads a "full chunk" flag and a varint mask where 1.17 reads a
    /// `BitSet`; everything behind them is the same container.
    #[test]
    fn the_1_16_mask_replaces_the_bit_set() {
        let v1_17 = to_1_17(&blob(24, stone()));
        let states = &MappingData::get()
            .step(JavaMinecraftVersion::V_1_17)
            .blockstates;
        let out = to_v1_16(&v1_17, states).expect("converted");

        let mut read: &[u8] = &out;
        assert_eq!(read.get_i32_be().unwrap(), 3);
        assert_eq!(read.get_i32_be().unwrap(), -2);
        assert!(read.get_bool().unwrap(), "full chunk");
        assert_eq!(read.get_var_int().unwrap().0, 1);

        let mut expected: &[u8] = &v1_17[8..];
        read_longs(&mut expected).unwrap();
        let mut heightmaps = Vec::new();
        copy_named_nbt(&mut read, &mut heightmaps).unwrap();
        let mut from_1_17 = Vec::new();
        copy_named_nbt(&mut expected, &mut from_1_17).unwrap();
        assert_eq!(heightmaps, from_1_17);
    }

    /// A world shorter than the client's own still owes 1024 biomes.
    #[test]
    fn a_short_world_pads_the_biome_column() {
        let out = to_v1_17(
            &after_id_pass(&blob(16, stone())),
            0,
            &MappingData::get().step(LAYOUT).blockstates,
            LAYOUT,
        )
        .expect("converted");
        let mut read: &[u8] = &out;
        read.get_i32_be().unwrap();
        read.get_i32_be().unwrap();
        read_longs(&mut read).unwrap();
        let mut heightmaps = Vec::new();
        copy_named_nbt(&mut read, &mut heightmaps).unwrap();
        assert_eq!(
            read.get_var_int().unwrap().0,
            i32::try_from(BIOMES_PER_COLUMN).unwrap()
        );
    }

    #[test]
    fn a_payload_that_is_not_whole_is_dropped() {
        let payload = after_id_pass(&blob(24, stone()));
        let states = &MappingData::get().step(LAYOUT).blockstates;

        for cut in [9, 12, 40, payload.len() - 1] {
            assert!(
                to_v1_17(&payload[..cut], WORLD_BOTTOM, states, LAYOUT).is_none(),
                "cut at {cut}"
            );
        }
        let mut trailing = payload.clone();
        trailing.push(0xff);
        assert!(
            to_v1_17(&trailing, WORLD_BOTTOM, states, LAYOUT).is_none(),
            "a byte left over means this is not the layout it was taken for"
        );
    }

    /// The other steps below 1.18 carry no block state table at all, so those
    /// two handlers are the whole chain for a chunk.
    #[test]
    fn only_two_steps_below_1_18_touch_block_states() {
        for from in [
            JavaMinecraftVersion::V_1_17_1,
            JavaMinecraftVersion::V_1_16_4,
            JavaMinecraftVersion::V_1_16_3,
        ] {
            assert!(
                MappingData::get().step(from).blockstates.is_empty(),
                "{from}"
            );
        }
        for from in [JavaMinecraftVersion::V_1_18, JavaMinecraftVersion::V_1_17] {
            assert!(
                !MappingData::get().step(from).blockstates.is_empty(),
                "{from}"
            );
        }
    }

    /// End to end: the packet core writes for an old client, through the id
    /// pass and both step handlers.
    #[test]
    fn the_pipeline_reframes_a_chunk_for_1_17_1_and_1_16_4() {
        const PLAY: u8 = 5;

        for (key, version, bit_set_mask) in [
            (91u64, JavaMinecraftVersion::V_1_17_1, true),
            (92, JavaMinecraftVersion::V_1_16_4, false),
        ] {
            let payload = chunk_packet(
                LAYOUT,
                &blob(24, stone()),
                i32::try_from(native_chest()).unwrap(),
            );
            with_connection(key, version, |connection| {
                connection.entity_tracker.min_y = WORLD_BOTTOM;
            });
            let out = crate::pipeline::translate_clientbound(
                key,
                version,
                PLAY,
                clientbound::play::LEVEL_CHUNK_WITH_LIGHT.v26_3,
                &payload,
            )
            .unwrap_or_else(|| panic!("{version} chunk translated"));
            remove_connection(key);

            let mut read: &[u8] = &out.payload;
            assert_eq!(read.get_i32_be().unwrap(), 3);
            assert_eq!(read.get_i32_be().unwrap(), -2);
            if bit_set_mask {
                assert_eq!(read_longs(&mut read).unwrap(), vec![1], "{version}");
            } else {
                assert!(read.get_bool().unwrap(), "{version} full chunk");
                assert_eq!(read.get_var_int().unwrap().0, 1, "{version}");
            }
            let mut heightmaps = Vec::new();
            copy_named_nbt(&mut read, &mut heightmaps).unwrap();
            assert_eq!(
                read.get_var_int().unwrap().0,
                i32::try_from(BIOMES_PER_COLUMN).unwrap(),
                "{version}"
            );
            for _ in 0..BIOMES_PER_COLUMN {
                read.get_var_int().unwrap();
            }

            let blob_len = usize::try_from(read.get_var_int().unwrap().0).unwrap();
            let (mut sections, mut rest) = read.split_at(blob_len);
            assert_eq!(sections.get_i16_be().unwrap(), 4096, "{version}");
            assert_eq!(sections.get_u8().unwrap(), LEGACY_BLOCK_BITS, "{version}");
            assert_eq!(sections.get_var_int().unwrap().0, 1, "{version}");
            let state = u16::try_from(sections.get_var_int().unwrap().0).unwrap();
            assert_eq!(
                state,
                crate::remap::block_state_remap::remap_block_state_for_version(
                    u16::try_from(stone()).unwrap(),
                    version
                ),
                "{version}: this client's own stone"
            );

            assert_eq!(rest.get_var_int().unwrap().0, 1, "{version}");
            let entity = read_named_compound(&mut rest).unwrap();
            assert_eq!(
                entity.get_string("id"),
                Some("minecraft:chest"),
                "{version}"
            );
            assert!(rest.is_empty(), "{version}");
        }
    }
}
