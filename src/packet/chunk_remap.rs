//! Remaps block state ids inside a `LEVEL_CHUNK_WITH_LIGHT` payload.
//!
//! Core writes the chunk in the client's own wire layout but the section
//! palette state ids are always 26.3's, and a client throws on an id its own
//! registry lacks, so the palettes are renumbered here.
//!
//! Only the block palette is touched; biome ids and the surrounding bytes
//! (heightmaps, light) are copied through untouched. Clients below 1.18 get
//! the shape reframed by [`crate::packet::chunk_legacy`] instead.
//!
//! Layout branches here are 1.20.2 (NBT root name), 1.21.5 (heightmap list,
//! no length prefixes) and 26.1 (fluid count).

use std::collections::HashSet;
use std::io::Cursor;

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::block::rewrite_chunk_block_entities;
use crate::api::types::{NbtT, WireType};
use crate::remap::block_state_remap::remap_block_state_for_version;

/// Oldest layout this parser understands, which is the one core writes.
pub const OLDEST_LAYOUT: JavaMinecraftVersion = JavaMinecraftVersion::V_1_18;

/// Highest `bits_per_entry` that still uses an indirect (listed) block palette.
/// Above this the section uses the direct palette and stores raw ids.
const MAX_INDIRECT_BLOCK_BITS: u8 = 8;
/// Same threshold for biome containers, which hold 4x4x4 entries.
const MAX_INDIRECT_BIOME_BITS: u8 = 3;
/// Block entries in a section.
const BLOCKS_PER_SECTION: usize = 16 * 16 * 16;
/// Biome entries in a section, from 1.18.
const BIOMES_PER_SECTION: usize = 4 * 4 * 4;
/// Bounds synthesized legacy block entities within Pumpkin's packet-size cap.
const MAX_CHUNK_BLOCK_ENTITIES: usize = 131_072;

/// Number of packed longs for `entry_count` entries at `bits_per_entry`.
const fn packed_long_count(entry_count: usize, bits_per_entry: u8) -> usize {
    if bits_per_entry == 0 {
        return 0;
    }
    let per_long = 64 / bits_per_entry as usize;
    entry_count.div_ceil(per_long)
}

/// Copies one palette container, remapping block state ids when `remap` is set.
/// Before 1.21.5 the packed data array carries a `VarInt` length; from 1.21.5 it's implied.
/// Direct palettes are rewritten in place at the sender's width; a mapped
/// state that cannot fit that width makes the packet fail closed.
fn copy_container(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    entry_count: usize,
    max_indirect_bits: u8,
    remap: Option<JavaMinecraftVersion>,
    version: JavaMinecraftVersion,
) -> Option<()> {
    let length_prefixed = version < JavaMinecraftVersion::V_1_21_5;
    let bits_per_entry = cursor.get_u8().ok()?;
    if bits_per_entry > 32 {
        return None;
    }
    out.write_u8(bits_per_entry).ok()?;

    if bits_per_entry == 0 {
        let id = cursor.get_var_int().ok()?.0;
        let id = match remap {
            Some(version) => i32::from(remap_block_state_for_version(
                u16::try_from(id).ok()?,
                version,
            )),
            None => id,
        };
        out.write_var_int(&VarInt(id)).ok()?;
    } else if bits_per_entry <= max_indirect_bits {
        let len = cursor.get_var_int().ok()?.0;
        out.write_var_int(&VarInt(len)).ok()?;
        for _ in 0..len {
            let id = cursor.get_var_int().ok()?.0;
            let id = match remap {
                Some(version) => i32::from(remap_block_state_for_version(
                    u16::try_from(id).ok()?,
                    version,
                )),
                None => id,
            };
            out.write_var_int(&VarInt(id)).ok()?;
        }
    }

    let expected_longs = packed_long_count(entry_count, bits_per_entry);
    let longs = if length_prefixed {
        let len = cursor.get_var_int().ok()?.0;
        out.write_var_int(&VarInt(len)).ok()?;
        let len = usize::try_from(len).ok()?;
        if len != expected_longs {
            return None;
        }
        len
    } else {
        expected_longs
    };
    let direct_remap = remap.is_some() && bits_per_entry > max_indirect_bits;
    let per_long = if bits_per_entry == 0 {
        0
    } else {
        64 / usize::from(bits_per_entry)
    };
    let mask = if bits_per_entry == 0 {
        0
    } else {
        (1u64 << bits_per_entry) - 1
    };
    for long_index in 0..longs {
        let packed = cursor.get_i64_be().ok()?;
        let packed = if direct_remap {
            let version = remap?;
            let mut rewritten = 0u64;
            for slot in 0..per_long {
                let entry_index = long_index * per_long + slot;
                if entry_index >= entry_count {
                    break;
                }
                let state =
                    u16::try_from((packed as u64 >> (slot * usize::from(bits_per_entry))) & mask)
                        .ok()?;
                let mapped = u64::from(remap_block_state_for_version(state, version));
                if mapped > mask {
                    return None;
                }
                rewritten |= mapped << (slot * usize::from(bits_per_entry));
            }
            rewritten as i64
        } else {
            packed
        };
        out.write_i64_be(packed).ok()?;
    }

    Some(())
}

/// Copies the heightmaps as `version` frames them: a list of (type, long array)
/// from 1.21.5, a network NBT compound before that (with a root name before
/// 1.20.2, which is where core's `write_nbt_with_version` drops it too, so
/// 764 and 765 take the unnamed branch).
fn copy_heightmaps(
    cursor: &mut &[u8],
    out: &mut Vec<u8>,
    version: JavaMinecraftVersion,
) -> Option<()> {
    if version >= JavaMinecraftVersion::V_1_21_5 {
        let map_count = cursor.get_var_int().ok()?.0;
        out.write_var_int(&VarInt(map_count)).ok()?;
        for _ in 0..map_count {
            let index = cursor.get_var_int().ok()?.0;
            let len = cursor.get_var_int().ok()?.0;
            out.write_var_int(&VarInt(index)).ok()?;
            out.write_var_int(&VarInt(len)).ok()?;
            for _ in 0..len {
                let val = cursor.get_i64_be().ok()?;
                out.write_i64_be(val).ok()?;
            }
        }
        return Some(());
    }

    let start = *cursor;
    let tag_id = cursor.get_u8().ok()?;
    if tag_id != 0 {
        if version < JavaMinecraftVersion::V_1_20_2 {
            // Named root: u16 length followed by the name bytes.
            let name_len = usize::try_from(cursor.get_i16_be().ok()?).ok()?;
            if cursor.len() < name_len {
                return None;
            }
            *cursor = &cursor[name_len..];
        }
        let mut nbt_cursor = Cursor::new(*cursor);
        let mut reader = NbtReadHelperJava::new(&mut nbt_cursor);
        NbtTag::skip_data(&mut reader, tag_id).ok()?;
        let used = usize::try_from(nbt_cursor.position()).ok()?;
        *cursor = &cursor[used..];
    }
    let consumed = start.len() - cursor.len();
    out.extend_from_slice(&start[..consumed]);
    Some(())
}

/// Rewrites the section blob of a chunk packet for `version`. `None` means the caller must drop the packet.
#[must_use]
pub fn remap_chunk_payload(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    if version < OLDEST_LAYOUT {
        return None;
    }

    let mut cursor = payload;
    let mut out = Vec::with_capacity(payload.len());

    // Chunk position.
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;
    out.write_i32_be(chunk_x).ok()?;
    out.write_i32_be(chunk_z).ok()?;

    copy_heightmaps(&mut cursor, &mut out, version)?;

    // Section blob, length prefixed.
    let data_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if data_len > cursor.len() {
        return None;
    }
    let (sections, rest) = cursor.split_at(data_len);

    let mut section_cursor = sections;
    let mut sections_out = Vec::with_capacity(sections.len());
    while !section_cursor.is_empty() {
        // core pads 1.21.5 chunk data with trailing zeros, copy an all-zero tail through untouched
        if section_cursor.iter().all(|&b| b == 0) {
            sections_out.extend_from_slice(section_cursor);
            break;
        }
        let block_count = section_cursor.get_i16_be().ok()?;
        sections_out.write_i16_be(block_count).ok()?;

        if version >= JavaMinecraftVersion::V_26_1 {
            // Fluid count, added in 26.1.
            let liquid_count = section_cursor.get_i16_be().ok()?;
            sections_out.write_i16_be(liquid_count).ok()?;
        }

        copy_container(
            &mut section_cursor,
            &mut sections_out,
            BLOCKS_PER_SECTION,
            MAX_INDIRECT_BLOCK_BITS,
            Some(version),
            version,
        )?;
        copy_container(
            &mut section_cursor,
            &mut sections_out,
            BIOMES_PER_SECTION,
            MAX_INDIRECT_BIOME_BITS,
            None,
            version,
        )?;
    }

    out.write_var_int(&VarInt(i32::try_from(sections_out.len()).ok()?))
        .ok()?;
    out.extend_from_slice(&sections_out);
    // The light data behind the block entities needs no renumbering.
    out.extend_from_slice(&rewrite_chunk_block_entities(rest, version)?);

    Some(out)
}

/// Finds block coordinates in a target-layout chunk whose remapped state IDs
/// satisfy `matches`. The chunk's section data has already passed through
/// [`remap_chunk_payload`], so palette IDs are in `version`'s registry.
#[must_use]
pub fn matching_block_positions(
    payload: &[u8],
    version: JavaMinecraftVersion,
    min_y: i32,
    mut matches: impl FnMut(i32) -> bool,
) -> Option<Vec<i64>> {
    if version < OLDEST_LAYOUT {
        return None;
    }

    let mut cursor = payload;
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;
    let mut ignored = Vec::new();
    copy_heightmaps(&mut cursor, &mut ignored, version)?;
    let data_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if data_len > cursor.len() {
        return None;
    }
    let (mut sections, _) = cursor.split_at(data_len);
    let mut positions = Vec::new();
    let mut section_index = 0i64;

    while !sections.is_empty() {
        // 1.21.5+ core data may include zero padding after the final section.
        if sections.iter().all(|&byte| byte == 0) {
            break;
        }
        sections.get_i16_be().ok()?; // non-air block count
        if version >= JavaMinecraftVersion::V_26_1 {
            sections.get_i16_be().ok()?; // fluid count
        }

        let found = matching_indices_in_block_palette(&mut sections, version, &mut matches)?;
        let mut ignored_biomes = Vec::new();
        copy_container(
            &mut sections,
            &mut ignored_biomes,
            BIOMES_PER_SECTION,
            MAX_INDIRECT_BIOME_BITS,
            None,
            version,
        )?;

        let section_y = i64::from(min_y) + (section_index << 4);
        for index in found {
            if positions.len() >= MAX_CHUNK_BLOCK_ENTITIES {
                return None;
            }
            let local_x = (index & 0x0f) as i64;
            let local_z = ((index >> 4) & 0x0f) as i64;
            let local_y = ((index >> 8) & 0x0f) as i64;
            positions.push(pack_block_position(
                i64::from(chunk_x) * 16 + local_x,
                section_y + local_y,
                i64::from(chunk_z) * 16 + local_z,
            ));
        }
        section_index += 1;
    }

    Some(positions)
}

/// Adds legacy block-entity entries to a target-layout chunk packet, keeping
/// the light data and existing entity payloads intact. `additions` contains
/// packed block positions and block-entity type IDs for `version`.
#[must_use]
pub fn append_chunk_block_entities(
    payload: &[u8],
    version: JavaMinecraftVersion,
    additions: &[(i64, i32)],
) -> Option<Vec<u8>> {
    if version < OLDEST_LAYOUT || additions.len() > MAX_CHUNK_BLOCK_ENTITIES {
        return None;
    }
    let mut cursor = payload;
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;
    let mut out = Vec::with_capacity(payload.len().saturating_add(additions.len() * 8));
    out.write_i32_be(chunk_x).ok()?;
    out.write_i32_be(chunk_z).ok()?;
    copy_heightmaps(&mut cursor, &mut out, version)?;
    let section_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if section_len > cursor.len() {
        return None;
    }
    let (sections, rest) = cursor.split_at(section_len);
    out.write_var_int(&VarInt(i32::try_from(section_len).ok()?))
        .ok()?;
    out.extend_from_slice(sections);
    cursor = rest;
    let old_count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if old_count > MAX_CHUNK_BLOCK_ENTITIES
        || old_count.saturating_add(additions.len()) > MAX_CHUNK_BLOCK_ENTITIES
    {
        return None;
    }
    let mut entries_cursor = cursor;
    let mut entries = Vec::new();
    let mut positions = HashSet::with_capacity(old_count.saturating_add(additions.len()));
    let nbt = crate::api::rewriter::block::RawNbtT::for_version(version);
    for _ in 0..old_count {
        let packed_xz = entries_cursor.get_u8().ok()?;
        let y = entries_cursor.get_i16_be().ok()?;
        let entity_type = entries_cursor.get_var_int().ok()?;
        let raw_nbt = nbt.read(&mut entries_cursor).ok()?;
        let x = chunk_x
            .checked_mul(16)?
            .checked_add(i32::from(packed_xz >> 4))?;
        let z = chunk_z
            .checked_mul(16)?
            .checked_add(i32::from(packed_xz & 0x0f))?;
        positions.insert(pack_block_position(
            i64::from(x),
            i64::from(y),
            i64::from(z),
        ));
        entries.write_u8(packed_xz).ok()?;
        entries.write_i16_be(y).ok()?;
        entries.write_var_int(&entity_type).ok()?;
        nbt.write(&mut entries, &raw_nbt).ok()?;
    }

    let mut empty_nbt = Vec::new();
    NbtT::for_version(version)
        .write(&mut empty_nbt, &Some(NbtTag::Compound(NbtCompound::new())))
        .ok()?;
    let mut added = 0usize;
    for &(position, entity_type) in additions {
        if positions.contains(&position) {
            continue;
        }
        let (x, y, z) = unpack_block_position(position);
        if x.div_euclid(16) != chunk_x || z.div_euclid(16) != chunk_z {
            return None;
        }
        entries
            .write_u8((((x & 0x0f) << 4) | (z & 0x0f)) as u8)
            .ok()?;
        entries.write_i16_be(i16::try_from(y).ok()?).ok()?;
        entries.write_var_int(&VarInt(entity_type)).ok()?;
        nbt.write(&mut entries, &empty_nbt).ok()?;
        positions.insert(position);
        added += 1;
    }

    out.write_var_int(&VarInt(i32::try_from(old_count + added).ok()?))
        .ok()?;
    out.extend_from_slice(&entries);
    out.extend_from_slice(entries_cursor);
    Some(out)
}

fn matching_indices_in_block_palette(
    cursor: &mut &[u8],
    version: JavaMinecraftVersion,
    matches: &mut impl FnMut(i32) -> bool,
) -> Option<Vec<usize>> {
    let bits_per_entry = cursor.get_u8().ok()?;
    let indirect = bits_per_entry <= MAX_INDIRECT_BLOCK_BITS;
    let mut palette = Vec::new();
    if bits_per_entry == 0 || indirect {
        let palette_len = if bits_per_entry == 0 {
            1
        } else {
            usize::try_from(cursor.get_var_int().ok()?.0).ok()?
        };
        if palette_len == 0 || palette_len > BLOCKS_PER_SECTION {
            return None;
        }
        for _ in 0..palette_len {
            palette.push(cursor.get_var_int().ok()?.0);
        }
    } else if bits_per_entry > 32 {
        return None;
    }

    let long_count = if version < JavaMinecraftVersion::V_1_21_5 {
        usize::try_from(cursor.get_var_int().ok()?.0).ok()?
    } else {
        packed_long_count(BLOCKS_PER_SECTION, bits_per_entry)
    };
    if long_count > packed_long_count(BLOCKS_PER_SECTION, bits_per_entry) {
        return None;
    }
    let mut words = Vec::with_capacity(long_count);
    for _ in 0..long_count {
        words.push(cursor.get_i64_be().ok()? as u64);
    }

    if bits_per_entry == 0 {
        return Some(if matches(palette[0]) {
            (0..BLOCKS_PER_SECTION).collect()
        } else {
            Vec::new()
        });
    }

    let per_long = 64 / usize::from(bits_per_entry);
    let mask = (1u64 << bits_per_entry) - 1;
    let mut found = Vec::new();
    for index in 0..BLOCKS_PER_SECTION {
        let word = *words.get(index / per_long)?;
        let palette_index =
            ((word >> ((index % per_long) * usize::from(bits_per_entry))) & mask) as usize;
        let state = if indirect {
            *palette.get(palette_index)?
        } else {
            i32::try_from(palette_index).ok()?
        };
        if matches(state) {
            found.push(index);
        }
    }
    Some(found)
}

fn pack_block_position(x: i64, y: i64, z: i64) -> i64 {
    (((x as u64) & 0x03ff_ffff) << 38 | ((z as u64) & 0x03ff_ffff) << 12 | ((y as u64) & 0x0fff))
        as i64
}

fn unpack_block_position(position: i64) -> (i32, i32, i32) {
    let bits = position as u64;
    let raw_x = ((bits >> 38) & 0x03ff_ffff) as i32;
    let raw_z = ((bits >> 12) & 0x03ff_ffff) as i32;
    let raw_y = (bits & 0x0fff) as i32;
    let x = (raw_x << 6) >> 6;
    let z = (raw_z << 6) >> 6;
    let y = (raw_y << 20) >> 20;
    (x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_1_21_4(stone: i32) -> Vec<u8> {
        let mut sections = Vec::new();
        // Section 1: 4096 blocks, single palette.
        sections.write_i16_be(4096).unwrap();
        sections.write_u8(0).unwrap();
        sections.write_var_int(&VarInt(stone)).unwrap();
        sections.write_var_int(&VarInt(0)).unwrap(); // packed data length
        sections.write_u8(0).unwrap(); // biome single palette
        sections.write_var_int(&VarInt(1)).unwrap();
        sections.write_var_int(&VarInt(0)).unwrap();
        // Section 2: indirect palette [air, stone], 4 bits per entry.
        sections.write_i16_be(1).unwrap();
        sections.write_u8(4).unwrap();
        sections.write_var_int(&VarInt(2)).unwrap();
        sections.write_var_int(&VarInt(0)).unwrap();
        sections.write_var_int(&VarInt(stone)).unwrap();
        sections.write_var_int(&VarInt(256)).unwrap();
        for _ in 0..256 {
            sections.write_i64_be(0).unwrap();
        }
        sections.write_u8(0).unwrap();
        sections.write_var_int(&VarInt(1)).unwrap();
        sections.write_var_int(&VarInt(0)).unwrap();

        let mut payload = Vec::new();
        payload.write_i32_be(3).unwrap();
        payload.write_i32_be(-4).unwrap();
        // Empty unnamed compound.
        payload.write_u8(10).unwrap();
        payload.write_u8(0).unwrap();
        payload
            .write_var_int(&VarInt(i32::try_from(sections.len()).unwrap()))
            .unwrap();
        payload.extend_from_slice(&sections);
        // Block entities: none. Light: stand-in bytes for the test.
        payload.write_var_int(&VarInt(0)).unwrap();
        payload.extend_from_slice(&[7, 7, 7]);
        payload
    }

    #[test]
    fn finds_matching_positions_in_a_single_value_section() {
        let version = JavaMinecraftVersion::V_1_21_4;
        let stone = i32::from(pumpkin_data::Block::STONE.default_state.id.as_u16());
        let payload = remap_chunk_payload(&chunk_1_21_4(stone), version).unwrap();
        let mapped = i32::from(remap_block_state_for_version(
            u16::try_from(stone).unwrap(),
            version,
        ));

        let found = matching_block_positions(&payload, version, -64, |state| state == mapped)
            .expect("remapped chunk should scan");
        assert_eq!(found.len(), BLOCKS_PER_SECTION);
        assert!(found.contains(&pack_block_position(48, -64, -64)));
        assert!(found.contains(&pack_block_position(63, -49, -49)));
    }

    #[test]
    fn finds_a_matching_index_in_an_indirect_palette() {
        let mut palette = Vec::new();
        palette.write_u8(4).unwrap();
        palette.write_var_int(&VarInt(2)).unwrap();
        palette.write_var_int(&VarInt(0)).unwrap();
        palette.write_var_int(&VarInt(42)).unwrap();
        palette.write_var_int(&VarInt(256)).unwrap();
        palette.write_i64_be(1).unwrap(); // index zero selects palette entry one
        for _ in 1..256 {
            palette.write_i64_be(0).unwrap();
        }

        let mut cursor = palette.as_slice();
        let found = matching_indices_in_block_palette(
            &mut cursor,
            JavaMinecraftVersion::V_1_21_4,
            &mut |state| state == 42,
        )
        .expect("indirect palette should scan");
        assert!(cursor.is_empty());
        assert_eq!(found, [0]);
    }

    #[test]
    fn remaps_direct_block_states_without_changing_the_palette_width() {
        let version = JavaMinecraftVersion::V_1_21_4;
        let source = i32::from(pumpkin_data::Block::STONE.default_state.id.as_u16());
        let expected = remap_block_state_for_version(u16::try_from(source).unwrap(), version);
        let bits = 16;
        let per_long = 64 / usize::from(bits);
        let long_count = packed_long_count(BLOCKS_PER_SECTION, bits);

        let mut input = Vec::new();
        input.write_u8(bits).unwrap();
        input
            .write_var_int(&VarInt(i32::try_from(long_count).unwrap()))
            .unwrap();
        for long_index in 0..long_count {
            let mut packed = 0u64;
            for slot in 0..per_long {
                if long_index * per_long + slot >= BLOCKS_PER_SECTION {
                    break;
                }
                packed |= u64::try_from(source).unwrap() << (slot * usize::from(bits));
            }
            input.write_i64_be(packed as i64).unwrap();
        }

        let mut cursor = input.as_slice();
        let mut output = Vec::new();
        copy_container(
            &mut cursor,
            &mut output,
            BLOCKS_PER_SECTION,
            MAX_INDIRECT_BLOCK_BITS,
            Some(version),
            version,
        )
        .expect("direct palette should remap");
        assert!(cursor.is_empty());

        let mut rewritten = output.as_slice();
        assert_eq!(rewritten.get_u8().unwrap(), bits);
        assert_eq!(
            rewritten.get_var_int().unwrap().0,
            i32::try_from(long_count).unwrap()
        );
        let first_word = rewritten.get_i64_be().unwrap() as u64;
        assert_eq!(first_word & ((1u64 << bits) - 1), u64::from(expected));
    }

    #[test]
    fn remaps_palettes_in_the_1_21_4_layout() {
        let stone = i32::from(pumpkin_data::Block::STONE.default_state.id.as_u16());
        let payload = chunk_1_21_4(stone);
        let out = remap_chunk_payload(&payload, JavaMinecraftVersion::V_1_21_4)
            .expect("1.21.4 chunk must parse");
        assert_eq!(&out[out.len() - 3..], &[7, 7, 7], "tail copied through");

        let mapped = i32::from(remap_block_state_for_version(
            u16::try_from(stone).unwrap(),
            JavaMinecraftVersion::V_1_21_4,
        ));
        // Position, empty NBT (2 bytes), blob length, block count.
        let mut cursor = &out[8 + 2..];
        let _blob_len = cursor.get_var_int().unwrap();
        assert_eq!(cursor.get_i16_be().unwrap(), 4096);
        assert_eq!(cursor.get_u8().unwrap(), 0);
        assert_eq!(cursor.get_var_int().unwrap().0, mapped);
    }

    #[test]
    fn remaps_palettes_in_the_1_20_5_layout() {
        let stone = i32::from(pumpkin_data::Block::STONE.default_state.id.as_u16());
        let payload = chunk_1_21_4(stone);
        let out = remap_chunk_payload(&payload, JavaMinecraftVersion::V_1_20_5)
            .expect("1.20.5 chunk must parse");
        assert_eq!(&out[out.len() - 3..], &[7, 7, 7], "tail copied through");

        let mapped = i32::from(remap_block_state_for_version(
            u16::try_from(stone).unwrap(),
            JavaMinecraftVersion::V_1_20_5,
        ));
        let mut cursor = &out[8 + 2..];
        let _blob_len = cursor.get_var_int().unwrap();
        assert_eq!(cursor.get_i16_be().unwrap(), 4096);
        assert_eq!(cursor.get_u8().unwrap(), 0);
        assert_eq!(cursor.get_var_int().unwrap().0, mapped);
    }

    #[test]
    fn remaps_palettes_in_the_1_20_2_and_1_20_3_layouts() {
        let stone = i32::from(pumpkin_data::Block::STONE.default_state.id.as_u16());
        let payload = chunk_1_21_4(stone);
        for version in [
            JavaMinecraftVersion::V_1_20_2,
            JavaMinecraftVersion::V_1_20_3,
        ] {
            let out = remap_chunk_payload(&payload, version).expect("1.20.2/1.20.3 chunk parses");
            assert_eq!(&out[out.len() - 3..], &[7, 7, 7], "tail copied through");

            let mapped = i32::from(remap_block_state_for_version(
                u16::try_from(stone).unwrap(),
                version,
            ));
            let mut cursor = &out[8 + 2..];
            let _blob_len = cursor.get_var_int().unwrap();
            assert_eq!(cursor.get_i16_be().unwrap(), 4096);
            assert_eq!(cursor.get_u8().unwrap(), 0);
            assert_eq!(cursor.get_var_int().unwrap().0, mapped);
        }
    }

    #[test]
    fn unnamed_heightmap_root_starts_at_1_20_2() {
        let mut heightmaps = Vec::new();
        heightmaps.write_u8(10).unwrap(); // TAG_Compound
        heightmaps.write_u8(12).unwrap(); // TAG_Long_Array
        heightmaps.write_i16_be(15).unwrap();
        heightmaps.extend_from_slice(b"MOTION_BLOCKING");
        heightmaps.write_i32_be(2).unwrap();
        heightmaps.write_i64_be(0).unwrap();
        heightmaps.write_i64_be(0).unwrap();
        heightmaps.write_u8(0).unwrap(); // TAG_End

        let empty = chunk_1_21_4(1);
        let mut payload = Vec::new();
        payload.extend_from_slice(&empty[..8]);
        payload.extend_from_slice(&heightmaps);
        payload.extend_from_slice(&empty[8 + 2..]);

        for version in [
            JavaMinecraftVersion::V_1_20_2,
            JavaMinecraftVersion::V_1_20_3,
            JavaMinecraftVersion::V_1_20_5,
        ] {
            assert!(
                remap_chunk_payload(&payload, version).is_some(),
                "{version} reads an unnamed heightmaps root"
            );
        }
        assert!(
            remap_chunk_payload(&payload, JavaMinecraftVersion::V_1_20).is_none(),
            "1.20 expects a root name and must reject these bytes"
        );
    }

    #[test]
    fn rejects_layouts_older_than_1_18() {
        let payload = chunk_1_21_4(1);
        assert!(remap_chunk_payload(&payload, JavaMinecraftVersion::V_1_17_1).is_none());
        assert!(remap_chunk_payload(&payload, JavaMinecraftVersion::V_1_16_2).is_none());
    }

    #[test]
    fn truncated_payload_is_rejected_not_forwarded() {
        let payload = chunk_1_21_4(1);
        for cut in [9, 12, 20, payload.len() - 40] {
            assert!(
                remap_chunk_payload(&payload[..cut], JavaMinecraftVersion::V_1_21_4).is_none(),
                "cut at {cut}"
            );
        }
    }
}
