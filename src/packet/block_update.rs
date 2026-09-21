//! Remaps block state ids in the block-change packets.
//!
//! Chunk palettes are remapped on load, but later changes arrive through these
//! packets carrying a raw state id, so they need the same remap.
//!
//! `multi_block_change`'s bool is `notTrustEdges` up to 1.19.2 and
//! `suppressLightUpdates` on 1.19.3/1.19.4 (renamed, same byte), dropped from 1.20.
//! Records are packed varlongs (`state << 12 | localPos`); minecraft-data types them
//! as varint from 1.19 but the wire bytes are identical since state ids never near 2^28.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::codec::var_long::VarLong;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::remap::block_state_remap::remap_block_state_for_version;

/// Oldest version these parsers understand; below 1.16 block position was packed differently.
pub const OLDEST_LAYOUT: JavaMinecraftVersion = JavaMinecraftVersion::V_1_16;

/// `BLOCK_UPDATE`: block position followed by the new state id.
#[must_use]
pub fn remap_block_update(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let position = cursor.get_i64_be().ok()?;
    let state_id = cursor.get_var_int().ok()?.0;
    if !cursor.is_empty() {
        return None;
    }

    let remapped = remap_block_state_for_version(u16::try_from(state_id).ok()?, version);

    let mut out = Vec::with_capacity(payload.len());
    out.write_i64_be(position).ok()?;
    out.write_var_int(&VarInt(i32::from(remapped))).ok()?;
    Some(out)
}

/// `SECTION_BLOCKS_UPDATE`: section position, a light-updates flag on 1.16 through 1.19.4, then packed varlongs.
#[must_use]
pub fn remap_section_blocks_update(
    payload: &[u8],
    version: JavaMinecraftVersion,
) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let section = cursor.get_i64_be().ok()?;
    let has_light_flag =
        version >= JavaMinecraftVersion::V_1_16 && version <= JavaMinecraftVersion::V_1_19_4;
    let suppress_light = if has_light_flag {
        Some(cursor.get_bool().ok()?)
    } else {
        None
    };
    let count = cursor.get_var_int().ok()?.0;

    let mut out = Vec::with_capacity(payload.len());
    out.write_i64_be(section).ok()?;
    if let Some(flag) = suppress_light {
        out.write_bool(flag).ok()?;
    }
    out.write_var_int(&VarInt(count)).ok()?;

    for _ in 0..count {
        let packed = cursor.get_var_long().ok()?.0 as u64;
        let local_pos = packed & 0xFFF;
        let state_id = u16::try_from(packed >> 12).ok()?;
        let remapped = remap_block_state_for_version(state_id, version);
        let repacked = (u64::from(remapped) << 12) | local_pos;
        out.write_var_long(&VarLong(repacked as i64)).ok()?;
    }
    if !cursor.is_empty() {
        return None;
    }

    Some(out)
}

/// Level event id of the block-break particle burst, whose data is a state id.
const PARTICLES_DESTROY_BLOCK: i32 = 2001;

/// `LEVEL_EVENT`: only the destroy-block event carries a state id, others pass through unchanged.
#[must_use]
pub fn remap_level_event(payload: &[u8], version: JavaMinecraftVersion) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let event = cursor.get_i32_be().ok()?;
    if event != PARTICLES_DESTROY_BLOCK {
        return Some(payload.to_vec());
    }
    let position = cursor.get_i64_be().ok()?;
    let data = cursor.get_i32_be().ok()?;
    let global = cursor.get_bool().ok()?;
    if !cursor.is_empty() {
        return None;
    }

    let remapped = remap_block_state_for_version(u16::try_from(data).ok()?, version);

    let mut out = Vec::with_capacity(payload.len());
    out.write_i32_be(event).ok()?;
    out.write_i64_be(position).ok()?;
    out.write_i32_be(i32::from(remapped)).ok()?;
    out.write_bool(global).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_update_keeps_light_flag_on_1_19_4_only() {
        let stone = pumpkin_data::Block::STONE.default_state.id.as_u16();
        let packed = (u64::from(stone) << 12) | 0x123;
        let mut with_flag = Vec::new();
        with_flag.write_i64_be(42).unwrap();
        with_flag.write_bool(true).unwrap();
        with_flag.write_var_int(&VarInt(1)).unwrap();
        with_flag.write_var_long(&VarLong(packed as i64)).unwrap();

        let out = remap_section_blocks_update(&with_flag, JavaMinecraftVersion::V_1_19_4)
            .expect("1.19.4 layout");
        assert_eq!(out[8], 1, "flag kept");

        assert!(remap_section_blocks_update(&with_flag, JavaMinecraftVersion::V_1_21_4).is_none());

        let mut without_flag = Vec::new();
        without_flag.write_i64_be(42).unwrap();
        without_flag.write_var_int(&VarInt(1)).unwrap();
        without_flag
            .write_var_long(&VarLong(packed as i64))
            .unwrap();
        let out = remap_section_blocks_update(&without_flag, JavaMinecraftVersion::V_1_21_4)
            .expect("1.21.4 layout");
        let mut cursor = &out[8..];
        assert_eq!(cursor.get_var_int().unwrap().0, 1);
        let repacked = cursor.get_var_long().unwrap().0 as u64;
        assert_eq!(repacked & 0xFFF, 0x123);
        assert_eq!(
            (repacked >> 12) as u16,
            remap_block_state_for_version(stone, JavaMinecraftVersion::V_1_21_4)
        );
    }

    #[test]
    fn section_update_and_level_event_parse_on_1_20_x() {
        for version in [
            JavaMinecraftVersion::V_1_20_2,
            JavaMinecraftVersion::V_1_20_3,
            JavaMinecraftVersion::V_1_20_5,
        ] {
            let stone = pumpkin_data::Block::STONE.default_state.id.as_u16();
            let packed = (u64::from(stone) << 12) | 0x123;
            let mut payload = Vec::new();
            payload.write_i64_be(42).unwrap();
            payload.write_var_int(&VarInt(1)).unwrap();
            payload.write_var_long(&VarLong(packed as i64)).unwrap();
            let out =
                remap_section_blocks_update(&payload, version).expect("1.20.x section update");
            let mut cursor = &out[8..];
            assert_eq!(cursor.get_var_int().unwrap().0, 1);
            let repacked = cursor.get_var_long().unwrap().0 as u64;
            assert_eq!(repacked & 0xFFF, 0x123);
            assert_eq!(
                (repacked >> 12) as u16,
                remap_block_state_for_version(stone, version)
            );

            let mut event = Vec::new();
            event.write_i32_be(PARTICLES_DESTROY_BLOCK).unwrap();
            event.write_i64_be(0).unwrap();
            event.write_i32_be(i32::from(stone)).unwrap();
            event.write_bool(false).unwrap();
            let out = remap_level_event(&event, version).expect("1.20.x level event");
            let mut cursor = &out[12..];
            assert_eq!(
                u16::try_from(cursor.get_i32_be().unwrap()).unwrap(),
                remap_block_state_for_version(stone, version)
            );

            let mut update = Vec::new();
            update.write_i64_be(7).unwrap();
            update.write_var_int(&VarInt(i32::from(stone))).unwrap();
            let out = remap_block_update(&update, version).expect("1.20.x block update");
            let mut cursor = &out[8..];
            assert_eq!(
                u16::try_from(cursor.get_var_int().unwrap().0).unwrap(),
                remap_block_state_for_version(stone, version)
            );
        }
    }

    #[test]
    fn all_three_layouts_hold_from_1_16_2_to_1_20_1() {
        let stone = pumpkin_data::Block::STONE.default_state.id.as_u16();
        let packed = (u64::from(stone) << 12) | 0x123;

        for version in [
            JavaMinecraftVersion::V_1_16_2,
            JavaMinecraftVersion::V_1_16_3,
            JavaMinecraftVersion::V_1_16_4,
            JavaMinecraftVersion::V_1_17,
            JavaMinecraftVersion::V_1_17_1,
            JavaMinecraftVersion::V_1_18,
            JavaMinecraftVersion::V_1_18_2,
            JavaMinecraftVersion::V_1_19,
            JavaMinecraftVersion::V_1_19_1,
            JavaMinecraftVersion::V_1_19_3,
            JavaMinecraftVersion::V_1_19_4,
            JavaMinecraftVersion::V_1_20,
        ] {
            let expect_flag = version <= JavaMinecraftVersion::V_1_19_4;

            let mut payload = Vec::new();
            payload.write_i64_be(42).unwrap();
            if expect_flag {
                payload.write_bool(true).unwrap();
            }
            payload.write_var_int(&VarInt(1)).unwrap();
            payload.write_var_long(&VarLong(packed as i64)).unwrap();

            let out = remap_section_blocks_update(&payload, version)
                .unwrap_or_else(|| panic!("{version} section update"));
            let mut cursor = &out[8..];
            if expect_flag {
                assert!(cursor.get_bool().unwrap(), "{version} keeps the light flag");
            }
            assert_eq!(cursor.get_var_int().unwrap().0, 1);
            let repacked = cursor.get_var_long().unwrap().0 as u64;
            assert_eq!(repacked & 0xFFF, 0x123, "{version} local position");
            assert_eq!(
                (repacked >> 12) as u16,
                remap_block_state_for_version(stone, version),
                "{version} state renumbered"
            );
            assert!(cursor.is_empty(), "{version} no trailing bytes");

            let mut update = Vec::new();
            update.write_i64_be(7).unwrap();
            update.write_var_int(&VarInt(i32::from(stone))).unwrap();
            let out = remap_block_update(&update, version)
                .unwrap_or_else(|| panic!("{version} block update"));
            let mut cursor = &out[8..];
            assert_eq!(
                u16::try_from(cursor.get_var_int().unwrap().0).unwrap(),
                remap_block_state_for_version(stone, version)
            );

            let mut event = Vec::new();
            event.write_i32_be(PARTICLES_DESTROY_BLOCK).unwrap();
            event.write_i64_be(0).unwrap();
            event.write_i32_be(i32::from(stone)).unwrap();
            event.write_bool(false).unwrap();
            let out = remap_level_event(&event, version)
                .unwrap_or_else(|| panic!("{version} level event"));
            let mut cursor = &out[12..];
            assert_eq!(
                u16::try_from(cursor.get_i32_be().unwrap()).unwrap(),
                remap_block_state_for_version(stone, version)
            );
            assert!(!cursor.get_bool().unwrap());
            assert!(cursor.is_empty());
        }
    }

    #[test]
    fn the_section_update_light_flag_ends_at_1_20() {
        let stone = pumpkin_data::Block::STONE.default_state.id.as_u16();
        let packed = (u64::from(stone) << 12) | 0x123;

        let mut with_flag = Vec::new();
        with_flag.write_i64_be(42).unwrap();
        with_flag.write_bool(true).unwrap();
        with_flag.write_var_int(&VarInt(1)).unwrap();
        with_flag.write_var_long(&VarLong(packed as i64)).unwrap();
        assert!(remap_section_blocks_update(&with_flag, JavaMinecraftVersion::V_1_20).is_none());

        let mut without_flag = Vec::new();
        without_flag.write_i64_be(42).unwrap();
        without_flag.write_var_int(&VarInt(1)).unwrap();
        without_flag
            .write_var_long(&VarLong(packed as i64))
            .unwrap();
        assert!(
            remap_section_blocks_update(&without_flag, JavaMinecraftVersion::V_1_19_4).is_none()
        );
    }

    #[test]
    fn level_event_without_state_passes_through() {
        let mut payload = Vec::new();
        payload.write_i32_be(1000).unwrap();
        payload.write_i64_be(0).unwrap();
        payload.write_i32_be(0).unwrap();
        payload.write_bool(false).unwrap();
        assert_eq!(
            remap_level_event(&payload, JavaMinecraftVersion::V_1_21_4).as_deref(),
            Some(payload.as_slice())
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut payload = Vec::new();
        payload.write_i64_be(0).unwrap();
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.push(0xff);
        assert!(remap_block_update(&payload, JavaMinecraftVersion::V_1_21_4).is_none());
    }
}
