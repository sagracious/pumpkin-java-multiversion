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
/// A light mask from the wire is a bit set, bounded by the protocol reader.
const MAX_LIGHT_MASK_LONGS: usize = 1024;
/// Light array counts are bounded independently of the packet's byte length.
const MAX_LIGHT_ARRAYS: i32 = 4096;

/// A 1.18 chunk rewritten for an older client, plus its detached light packet.
pub struct LegacyChunkOutput {
    pub chunk: Vec<u8>,
    pub light_update: Vec<u8>,
}

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

fn read_light_mask(cursor: &mut &[u8]) -> Option<Vec<i64>> {
    let count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if count > MAX_LIGHT_MASK_LONGS {
        return None;
    }
    let mut longs = Vec::with_capacity(count);
    for _ in 0..count {
        longs.push(cursor.get_i64_be().ok()?);
    }
    Some(longs)
}

fn light_mask_contains(mask: &[i64], bit: usize) -> bool {
    mask.get(bit / 64)
        .is_some_and(|word| ((*word as u64) & (1u64 << (bit % 64))) != 0)
}

fn read_light_arrays(cursor: &mut &[u8], mask: &[i64]) -> Option<Vec<Vec<u8>>> {
    let count = cursor.get_var_int().ok()?.0;
    if !(0..=MAX_LIGHT_ARRAYS).contains(&count) {
        return None;
    }
    let expected = mask
        .iter()
        .map(|word| (*word as u64).count_ones())
        .sum::<u32>();
    if usize::try_from(count).ok()? != usize::try_from(expected).ok()? {
        return None;
    }

    let mut arrays = Vec::with_capacity(usize::try_from(count).ok()?);
    for _ in 0..count {
        let len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
        if len > cursor.len() {
            return None;
        }
        let (array, rest) = cursor.split_at(len);
        arrays.push(array.to_vec());
        *cursor = rest;
    }
    Some(arrays)
}

/// Cuts the light sections to the 18 slots the pre-1.17 light packet exposes.
fn cut_light_mask(mask: &[i64], start: usize) -> Option<i32> {
    let mut cut = 0i32;
    for target_bit in 0..18usize {
        if light_mask_contains(mask, start.checked_add(target_bit)?) {
            cut |= 1i32 << target_bit;
        }
    }
    Some(cut)
}

fn cut_light_arrays(
    mask: &[i64],
    arrays: Vec<Vec<u8>>,
    start: usize,
) -> Option<(i32, Vec<Vec<u8>>)> {
    let end = start.checked_add(18)?;
    let mut input = arrays.into_iter();
    let mut cut_mask = 0i32;
    let mut cut_arrays = Vec::new();
    for bit in 0..mask.len().checked_mul(64)? {
        if !light_mask_contains(mask, bit) {
            continue;
        }
        let array = input.next()?;
        if (start..end).contains(&bit) {
            let target_bit = bit - start;
            cut_mask |= 1i32.checked_shl(u32::try_from(target_bit).ok()?)?;
            cut_arrays.push(array);
        }
    }
    input.next().is_none().then_some((cut_mask, cut_arrays))
}

fn write_light_arrays(out: &mut Vec<u8>, arrays: &[Vec<u8>]) -> Option<()> {
    for array in arrays {
        out.write_var_int(&VarInt(i32::try_from(array.len()).ok()?))
            .ok()?;
        out.extend_from_slice(array);
    }
    Some(())
}

/// Converts the bundled 1.18 light data directly to the negotiated client's
/// LIGHT_UPDATE layout. Extra packets bypass later protocol steps, so a 1.16
/// client must receive its own VarInt-mask form here.
fn light_update(
    cursor: &mut &[u8],
    chunk_x: i32,
    chunk_z: i32,
    world_bottom: i32,
    client_version: JavaMinecraftVersion,
) -> Option<Vec<u8>> {
    let trust_edges = cursor.get_bool().ok()?;
    let sky_mask = read_light_mask(cursor)?;
    let block_mask = read_light_mask(cursor)?;
    let empty_sky_mask = read_light_mask(cursor)?;
    let empty_block_mask = read_light_mask(cursor)?;
    let sky_arrays = read_light_arrays(cursor, &sky_mask)?;
    let block_arrays = read_light_arrays(cursor, &block_mask)?;

    let mut out = Vec::new();
    out.write_var_int(&VarInt(chunk_x)).ok()?;
    out.write_var_int(&VarInt(chunk_z)).ok()?;
    if client_version >= JavaMinecraftVersion::V_1_16
        && client_version <= JavaMinecraftVersion::V_1_19_4
    {
        out.write_bool(trust_edges).ok()?;
    }

    if client_version >= JavaMinecraftVersion::V_1_17 {
        for mask in [&sky_mask, &block_mask, &empty_sky_mask, &empty_block_mask] {
            write_longs(&mut out, mask)?;
        }
        out.write_var_int(&VarInt(i32::try_from(sky_arrays.len()).ok()?))
            .ok()?;
        write_light_arrays(&mut out, &sky_arrays)?;
        out.write_var_int(&VarInt(i32::try_from(block_arrays.len()).ok()?))
            .ok()?;
        write_light_arrays(&mut out, &block_arrays)?;
    } else {
        let start = usize::try_from((-(world_bottom >> 4)).max(0)).ok()?;
        let (sky_mask, sky_arrays) = cut_light_arrays(&sky_mask, sky_arrays, start)?;
        let (block_mask, block_arrays) = cut_light_arrays(&block_mask, block_arrays, start)?;
        let empty_sky_mask = cut_light_mask(&empty_sky_mask, start)?;
        let empty_block_mask = cut_light_mask(&empty_block_mask, start)?;
        for mask in [sky_mask, block_mask, empty_sky_mask, empty_block_mask] {
            out.write_var_int(&VarInt(mask)).ok()?;
        }
        write_light_arrays(&mut out, &sky_arrays)?;
        write_light_arrays(&mut out, &block_arrays)?;
    }
    Some(out)
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
    values: Vec<u32>,
    direct_bits: Option<u8>,
}

impl BlockContainer {
    /// The single value palette arrived with 1.18; below it vanilla rounds any
    /// width up to four and reads a palette list, so the one id becomes a one
    /// entry palette every block indexes.
    fn read(cursor: &mut &[u8]) -> Option<Self> {
        let bits = cursor.get_u8().ok()?;
        if bits == 0 {
            let id = u32::try_from(cursor.get_var_int().ok()?.0).ok()?;
            let packed = read_longs(cursor)?;
            if !packed.is_empty() {
                return None;
            }
            return Some(Self {
                values: vec![id; BLOCKS_PER_SECTION],
                direct_bits: None,
            });
        }
        if bits > 32 || bits < LEGACY_BLOCK_BITS {
            return None;
        }
        let palette = if bits <= MAX_INDIRECT_BLOCK_BITS {
            Some(read_palette(cursor)?)
        } else {
            None
        };
        let packed = read_longs(cursor)?;
        let indices = unpack(&packed, bits, BLOCKS_PER_SECTION)?;
        if packed.len() != BLOCKS_PER_SECTION.div_ceil(per_long(bits)) {
            return None;
        }
        let direct_bits = palette.is_none().then_some(bits);
        let values = match palette {
            Some(palette) => indices
                .iter()
                .map(|&index| palette.get(index as usize).copied())
                .collect::<Option<Vec<_>>>()?,
            None => indices,
        };
        Some(Self {
            values,
            direct_bits,
        })
    }

    fn write(&self, out: &mut Vec<u8>, states: &IdMapping, global_bits: Option<u8>) -> Option<()> {
        if self.values.len() != BLOCKS_PER_SECTION {
            return None;
        }

        let mapped: Vec<u32> = self
            .values
            .iter()
            .map(|&state| states.map(state).unwrap_or(0))
            .collect();
        let mut palette = Vec::new();
        let mut indices = std::collections::HashMap::new();
        for &state in &mapped {
            let next = u32::try_from(palette.len()).ok()?;
            let index = *indices.entry(state).or_insert_with(|| {
                palette.push(state);
                next
            });
            if index as usize >= palette.len() {
                return None;
            }
        }

        let indirect = palette.len() <= (1usize << MAX_INDIRECT_BLOCK_BITS);
        let bits = if indirect {
            LEGACY_BLOCK_BITS.max(
                u8::try_from(usize::BITS - (palette.len().saturating_sub(1)).leading_zeros())
                    .ok()?,
            )
        } else if global_bits.is_none() {
            self.direct_bits.unwrap_or_else(|| {
                let size = mapped.iter().copied().max().unwrap_or(0).saturating_add(1) as usize;
                u8::try_from(usize::BITS - (size.saturating_sub(1)).leading_zeros())
                    .unwrap_or(MAX_INDIRECT_BLOCK_BITS + 1)
                    .max(MAX_INDIRECT_BLOCK_BITS + 1)
            })
        } else {
            global_bits?
        };
        out.write_u8(bits).ok()?;

        let encoded: Vec<u32> = if indirect {
            out.write_var_int(&VarInt(i32::try_from(palette.len()).ok()?))
                .ok()?;
            for &state in &palette {
                out.write_var_int(&VarInt(i32::try_from(state).ok()?))
                    .ok()?;
            }
            mapped
                .iter()
                .map(|state| indices.get(state).copied())
                .collect::<Option<Vec<_>>>()?
        } else {
            mapped
        };

        let entries_per_long = per_long(bits);
        let mask = (1u64 << bits) - 1;
        let mut packed = vec![0u64; BLOCKS_PER_SECTION.div_ceil(entries_per_long)];
        for (index, value) in encoded.into_iter().enumerate() {
            if u64::from(value) > mask {
                return None;
            }
            packed[index / entries_per_long] |=
                u64::from(value) << (usize::from(bits) * (index % entries_per_long));
        }
        write_longs(
            out,
            &packed
                .into_iter()
                .map(|value| value as i64)
                .collect::<Vec<_>>(),
        )
    }
}

/// Global palette width is a property of the target version's registry, not
/// the maximum state present in this section. `None` is reserved for a true
/// identity mapping where no target registry size is available.
fn global_palette_bits(states: &IdMapping) -> Option<u8> {
    if states.is_identity() && states.is_empty() {
        return None;
    }
    let size = states.inverse().len();
    let bits = usize::BITS - (size.saturating_sub(1)).leading_zeros();
    Some(u8::try_from(bits).ok()?.max(MAX_INDIRECT_BLOCK_BITS + 1))
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
    source_layout: JavaMinecraftVersion,
    client_version: JavaMinecraftVersion,
) -> Option<LegacyChunkOutput> {
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
    let global_bits = global_palette_bits(states);
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
                blocks.write(&mut kept, states, global_bits)?;
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
    write_block_entities(&mut cursor, chunk_x, chunk_z, source_layout, &mut out)?;
    let light_update = light_update(&mut cursor, chunk_x, chunk_z, world_bottom, client_version)?;
    cursor.is_empty().then_some(LegacyChunkOutput {
        chunk: out,
        light_update,
    })
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
    let global_bits = global_palette_bits(states);

    let mut sections = blob;
    let mut rewritten = Vec::with_capacity(blob.len());
    for _ in 0..(mask as u64).count_ones() {
        let block_count = sections.get_i16_be().ok()?;
        rewritten.write_i16_be(block_count).ok()?;
        BlockContainer::read(&mut sections)?.write(&mut rewritten, states, global_bits)?;
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
    use crate::packet::mappings::clientbound::play::LIGHT_UPDATE;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::ServerPacket;
    use pumpkin_protocol::codec::bit_set::BitSet;
    use pumpkin_protocol::java::client::play::{
        CChunkData, CLightUpdate, ChunkBlockEntity, ChunkHeightmaps, LightData,
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

    fn direct_blob(count: usize, states: &[u32]) -> Vec<u8> {
        let max_state = states.iter().copied().max().expect("states");
        let bits = u8::try_from(u32::BITS - max_state.leading_zeros()).unwrap();
        assert!(bits > MAX_INDIRECT_BLOCK_BITS);
        let entries_per_long = per_long(bits);
        let mut blob = Vec::new();
        for _ in 0..count {
            blob.write_i16_be(4096).unwrap();
            blob.write_u8(bits).unwrap();
            let mut packed = vec![0u64; BLOCKS_PER_SECTION.div_ceil(entries_per_long)];
            for index in 0..BLOCKS_PER_SECTION {
                let value = u64::from(states[index % states.len()]);
                packed[index / entries_per_long] |=
                    value << (usize::from(bits) * (index % entries_per_long));
            }
            write_longs(
                &mut blob,
                &packed
                    .into_iter()
                    .map(|value| value as i64)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            blob.write_u8(0).unwrap();
            blob.write_var_int(&VarInt(0)).unwrap();
            blob.write_var_int(&VarInt(0)).unwrap();
        }
        blob
    }

    /// One chunk packet through upstream's own writer, in the layout core
    /// writes on every version.
    fn chunk_packet(version: JavaMinecraftVersion, data: &[u8], block_entity: i32) -> Vec<u8> {
        chunk_packet_with_light(version, data, block_entity, LightData::default())
    }

    fn chunk_packet_with_light(
        version: JavaMinecraftVersion,
        data: &[u8],
        block_entity: i32,
        light: LightData,
    ) -> Vec<u8> {
        let mut chest = NbtCompound::new();
        chest.put_string("CustomName", "crate".to_string());
        let packet = CChunkData::new(
            3,
            -2,
            ChunkHeightmaps::default(),
            data,
            vec![ChunkBlockEntity::new(0x21, 70, VarInt(block_entity), chest)],
            light,
        );
        let mut payload = Vec::new();
        packet.write_packet_data(&mut payload, &version).unwrap();
        payload
    }

    fn light_data() -> LightData {
        let mut sky_light_mask = BitSet::default();
        for bit in [3, 4, 21, 22] {
            sky_light_mask.set_bit(bit, true);
        }
        let mut block_light_mask = BitSet::default();
        for bit in [4, 22] {
            block_light_mask.set_bit(bit, true);
        }
        let mut empty_sky_light_mask = BitSet::default();
        for bit in [3, 20] {
            empty_sky_light_mask.set_bit(bit, true);
        }
        let mut empty_block_light_mask = BitSet::default();
        for bit in [21, 22] {
            empty_block_light_mask.set_bit(bit, true);
        }
        LightData::new(
            true,
            sky_light_mask,
            block_light_mask,
            empty_sky_light_mask,
            empty_block_light_mask,
            vec![
                vec![0x33; 2048],
                vec![0x44; 2048],
                vec![0x55; 2048],
                vec![0x66; 2048],
            ],
            vec![vec![0x77; 2048], vec![0x88; 2048]],
        )
    }

    fn light_data_for_1_16() -> LightData {
        let mut sky_light_mask = BitSet::default();
        sky_light_mask.set_bit(0, true);
        sky_light_mask.set_bit(17, true);
        let mut block_light_mask = BitSet::default();
        block_light_mask.set_bit(0, true);
        let mut empty_sky_light_mask = BitSet::default();
        empty_sky_light_mask.set_bit(16, true);
        let mut empty_block_light_mask = BitSet::default();
        empty_block_light_mask.set_bit(17, true);
        LightData::new(
            true,
            sky_light_mask,
            block_light_mask,
            empty_sky_light_mask,
            empty_block_light_mask,
            vec![vec![0x44; 2048], vec![0x55; 2048]],
            vec![vec![0x77; 2048]],
        )
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
            JavaMinecraftVersion::V_1_17_1,
        )
        .expect("converted")
        .chunk
    }

    fn first_block_bits(payload: &[u8], v1_16: bool) -> u8 {
        let mut read = payload;
        read.get_i32_be().unwrap();
        read.get_i32_be().unwrap();
        if v1_16 {
            assert!(read.get_bool().unwrap());
            read.get_var_int().unwrap();
        } else {
            read_longs(&mut read).unwrap();
        }
        let mut heightmaps = Vec::new();
        copy_named_nbt(&mut read, &mut heightmaps).unwrap();
        let biome_count = read.get_var_int().unwrap().0;
        for _ in 0..biome_count {
            read.get_var_int().unwrap();
        }
        let data_len = usize::try_from(read.get_var_int().unwrap().0).unwrap();
        let (mut sections, _) = read.split_at(data_len);
        sections.get_i16_be().unwrap();
        sections.get_u8().unwrap()
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

    #[test]
    fn direct_block_palettes_are_repacked_for_1_17_and_1_16() {
        let to_18 = MappingData::get().composed(LAYOUT).blockstates.inverse();
        let to_17 = &MappingData::get().step(LAYOUT).blockstates;
        let mut seen = std::collections::HashSet::new();
        let mut native_states = Vec::new();
        for state_18 in 0..to_18.len() as u32 {
            let Some(state_17) = to_17.map(state_18) else {
                continue;
            };
            if !seen.insert(state_17) {
                continue;
            }
            let Some(native_state) = to_18.map(state_18) else {
                continue;
            };
            native_states.push(native_state);
            if native_states.len() == 300 {
                break;
            }
        }
        assert_eq!(
            native_states.len(),
            300,
            "fixture covers more than 256 states"
        );

        let inverse = to_17.inverse();
        for id in 0..inverse.len() {
            assert!(
                inverse.map(u32::try_from(id).unwrap()).is_some(),
                "target 1.17 block-state ids are contiguous through {id}"
            );
        }

        let payload = after_id_pass(&direct_blob(24, &native_states));
        let states_18_to_17 = &MappingData::get().step(LAYOUT).blockstates;
        let v1_17 = to_v1_17(
            &payload,
            WORLD_BOTTOM,
            states_18_to_17,
            LAYOUT,
            JavaMinecraftVersion::V_1_17_1,
        )
        .expect("direct palette converts to 1.17")
        .chunk;
        assert!(
            first_block_bits(&v1_17, false) > MAX_INDIRECT_BLOCK_BITS,
            "more than 256 states use a direct palette"
        );

        let states_17_to_16 = &MappingData::get()
            .step(JavaMinecraftVersion::V_1_17)
            .blockstates;
        let inverse_17_to_16 = states_17_to_16.inverse();
        for id in 0..inverse_17_to_16.len() {
            assert!(
                inverse_17_to_16.map(u32::try_from(id).unwrap()).is_some(),
                "target 1.16 block-state ids are contiguous through {id}"
            );
        }
        let v1_16 = to_v1_16(&v1_17, states_17_to_16).expect("direct palette converts to 1.16.2");
        assert!(
            first_block_bits(&v1_16, true) > MAX_INDIRECT_BLOCK_BITS,
            "the 1.16 output keeps a direct palette"
        );
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
            JavaMinecraftVersion::V_1_17_1,
        )
        .expect("converted")
        .chunk;
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
                to_v1_17(
                    &payload[..cut],
                    WORLD_BOTTOM,
                    states,
                    LAYOUT,
                    JavaMinecraftVersion::V_1_17_1,
                )
                .is_none(),
                "cut at {cut}"
            );
        }
        let mut trailing = payload.clone();
        trailing.push(0xff);
        assert!(
            to_v1_17(
                &trailing,
                WORLD_BOTTOM,
                states,
                LAYOUT,
                JavaMinecraftVersion::V_1_17_1,
            )
            .is_none(),
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
            (93, JavaMinecraftVersion::V_1_16_2, false),
        ] {
            let payload = chunk_packet_with_light(
                LAYOUT,
                &blob(24, stone()),
                i32::try_from(native_chest()).unwrap(),
                light_data(),
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

            // Both outputs are extras and the original is cancelled, so every
            // host path sends these in the required light-then-chunk order.
            assert!(out.cancelled, "{version} replaces the bundled packet");
            assert_eq!(out.extra.len(), 2, "{version} emits light and chunk");
            assert_eq!(
                out.extra[0].0.to_id(version),
                LIGHT_UPDATE.to_id(version),
                "{version} light packet comes before the chunk"
            );
            assert_eq!(
                out.extra[1].0.to_id(version),
                clientbound::play::LEVEL_CHUNK_WITH_LIGHT.to_id(version),
                "{version} chunk follows its light update"
            );
            let mut light_payload = out.extra[0].1.as_slice();
            let light = CLightUpdate::read(&mut light_payload, &version)
                .unwrap_or_else(|_| panic!("{version} light update parses"));
            assert!(
                light_payload.is_empty(),
                "{version} light packet is complete"
            );
            assert_eq!(light.chunk_x.0, 3, "{version}");
            assert_eq!(light.chunk_z.0, -2, "{version}");
            assert_eq!(
                light.light_data,
                if bit_set_mask {
                    light_data()
                } else {
                    light_data_for_1_16()
                },
                "{version} light masks and arrays match its wire layout"
            );

            let mut read: &[u8] = &out.extra[1].1;
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
