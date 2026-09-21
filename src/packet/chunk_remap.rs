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

use std::io::Cursor;

use pumpkin_nbt::deserializer::NbtReadHelperJava;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::block::rewrite_chunk_block_entities;
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
/// Returns `None` for a direct palette, which is left alone rather than repacked at a different bit width.
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
    } else if remap.is_some() {
        // Direct palette: ids live in the packed data at a width derived from
        // the sender's registry size, so they cannot simply be copied across.
        return None;
    }

    let longs = if length_prefixed {
        let len = cursor.get_var_int().ok()?.0;
        out.write_var_int(&VarInt(len)).ok()?;
        usize::try_from(len).ok()?
    } else {
        packed_long_count(entry_count, bits_per_entry)
    };
    for _ in 0..longs {
        let packed = cursor.get_i64_be().ok()?;
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
