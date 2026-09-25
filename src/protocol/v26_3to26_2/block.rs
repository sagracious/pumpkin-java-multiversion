use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkReadSliceExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::{particle, sound};
use crate::api::types::{BOOL, I16, I64, NbtT, STRING, U8, VAR_INT, WireType};
use crate::api::{Ctx, MappingData, PacketWrapper, Registry, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};

const POT_DECORATION_SIDES: &[&str] = &["back", "left", "right", "front"];
const BREWING_STAND_MENU_TYPE: i32 = 11;

#[derive(Default)]
struct ScreenStorage {
    brewing_stand_container_id: Option<i32>,
}

pub(super) fn register(reg: &mut Registry) {
    reg.clientbound(&clientbound::play::BLOCK_ENTITY_DATA, block_entity_data);
    reg.clientbound(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, chunk);
    reg.clientbound_layout(&clientbound::play::LIGHT_UPDATE, light_update);
    reg.clientbound_layout(&clientbound::play::EXPLODE, explode);
    reg.clientbound_layout(&clientbound::play::OPEN_SIGN_EDITOR, open_sign_editor);
    reg.serverbound(&serverbound::play::SIGN_UPDATE, sign_update);
    reg.clientbound(&clientbound::play::OPEN_SCREEN, open_screen);
    reg.clientbound(&clientbound::play::CONTAINER_SET_DATA, container_set_data);
}

fn downgrade_pot_sherds(tag: Option<NbtTag>) -> Option<NbtTag> {
    let mut root = match tag {
        Some(NbtTag::Compound(root)) => root,
        other => return other,
    };
    let Some(NbtTag::Compound(decorations)) = root.child_tags.get("sherds") else {
        return Some(NbtTag::Compound(root));
    };

    let sherds = POT_DECORATION_SIDES
        .iter()
        .map(|side| {
            let item_id = decorations
                .get_compound(side)
                .and_then(|side| side.get_string("id"))
                .unwrap_or("minecraft:brick");
            NbtTag::String(item_id.to_string().into_boxed_str())
        })
        .collect();
    root.put_list("sherds", sherds);
    Some(NbtTag::Compound(root))
}

fn block_entity_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    // Block entity type ids replaced action bytes in 1.18. The server's older
    // layouts cannot contain a 26.3 pot sherd compound.
    if ctx.layout < V::V_1_18 {
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.passthrough(&I64)?; // block position
    wrapper.passthrough(&VAR_INT)?; // block entity type
    let nbt = wrapper.read(&NbtT::for_version(ctx.layout))?;
    wrapper.write(&NbtT::for_version(ctx.layout), &downgrade_pot_sherds(nbt))?;
    wrapper.passthrough_all();
    Ok(())
}

fn rewrite_chunk_block_entities(payload: &[u8], version: V) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let mut out = Vec::with_capacity(payload.len());
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;
    out.write_i32_be(chunk_x).ok()?;
    out.write_i32_be(chunk_z).ok()?;

    let heightmaps = NbtT::for_version(version).read(&mut cursor).ok()?;
    NbtT::for_version(version)
        .write(&mut out, &heightmaps)
        .ok()?;
    let section_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if section_len > cursor.len() {
        return None;
    }
    let (sections, rest) = cursor.split_at(section_len);
    cursor = rest;
    out.write_var_int(&VarInt(i32::try_from(section_len).ok()?))
        .ok()?;
    out.extend_from_slice(sections);

    let block_entity_count = cursor.get_var_int().ok()?.0;
    if !(0..=4096).contains(&block_entity_count) {
        return None;
    }
    out.write_var_int(&VarInt(block_entity_count)).ok()?;
    for _ in 0..block_entity_count {
        out.write_u8(cursor.get_u8().ok()?).ok()?;
        out.write_i16_be(cursor.get_i16_be().ok()?).ok()?;
        out.write_var_int(&cursor.get_var_int().ok()?).ok()?;
        let nbt = NbtT::for_version(version).read(&mut cursor).ok()?;
        NbtT::for_version(version)
            .write(&mut out, &downgrade_pot_sherds(nbt))
            .ok()?;
    }
    // Light arrays follow the block entity list. Their target layout is
    // already selected by Pumpkin's version-aware chunk writer.
    if version >= V::V_26_3 {
        for _ in 0..4 {
            bit_set_to_long_array(&mut cursor, &mut out)?;
        }
    }
    out.extend_from_slice(cursor);
    Some(out)
}

fn chunk(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V::V_1_18 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let translated = rewrite_chunk_block_entities(wrapper.remaining(), ctx.layout)
        .ok_or(TranslateError::Unsupported("chunk block entity NBT"))?;
    wrapper.replace_remaining(translated);
    Ok(())
}

/// Converts the 26.3 byte-backed bitset to the long-array representation.
/// `clientbound_layout` ensures this only runs when the incoming payload is
/// still 26.3 layout; Pumpkin already writes masks for older negotiated clients.
fn bit_set_to_long_array(cursor: &mut &[u8], out: &mut Vec<u8>) -> Option<()> {
    let byte_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if byte_len > 8192 {
        return None;
    }
    let bytes = cursor.read_slice_borrowed(byte_len).ok()?;
    let mut words = Vec::with_capacity(byte_len.div_ceil(8));
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        words.push(i64::from_le_bytes(word));
    }
    while words.last() == Some(&0) {
        words.pop();
    }
    out.write_var_int(&VarInt(i32::try_from(words.len()).ok()?))
        .ok()?;
    for word in words {
        out.write_i64_be(word).ok()?;
    }
    Some(())
}

fn light_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let mut cursor = wrapper.remaining();
    let mut out = Vec::with_capacity(cursor.len());
    out.write_var_int(&cursor.get_var_int()?).ok()?;
    out.write_var_int(&cursor.get_var_int()?).ok()?;
    for _ in 0..4 {
        bit_set_to_long_array(&mut cursor, &mut out)
            .ok_or(TranslateError::Unsupported("light bitset"))?;
    }
    out.extend_from_slice(cursor);
    wrapper.replace_remaining(out);
    Ok(())
}

fn rewrite_explosion_payload(payload: &[u8], target: V) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let mut out = Vec::with_capacity(payload.len());
    let ids = crate::api::MappingData::get().composed(target);

    for _ in 0..3 {
        let value = cursor.get_f64_be().ok()?;
        out.write_f64_be(value).ok()?;
    }
    out.write_f32_be(cursor.get_f32_be().ok()?).ok()?;
    out.write_i32_be(cursor.get_i32_be().ok()?).ok()?;

    let has_knockback = cursor.get_bool().ok()?;
    out.write_bool(has_knockback).ok()?;
    if has_knockback {
        for _ in 0..3 {
            out.write_f64_be(cursor.get_f64_be().ok()?).ok()?;
        }
    }

    let particle = particle::read_particle(&mut cursor).ok()?;
    if !particle::write_particle(&mut out, &particle, target, ids).ok()? {
        return None;
    }

    let sound_start = payload.len() - cursor.len();
    let holder = cursor.get_var_int().ok()?.0;
    if holder == 0 {
        cursor.get_str().ok()?;
        if cursor.get_bool().ok()? {
            cursor.get_f32_be().ok()?;
        }
    }
    let sound_end = payload.len() - cursor.len();
    {
        // The block-particle pool follows the sound holder; preserve its count.
        let source_sound = &payload[sound_start..sound_end];
        let mut probe = cursor;
        let block_particle_pool = probe.get_var_int().ok()?.0;
        let flag = probe.get_bool().ok()?;
        if !probe.is_empty() || block_particle_pool < 0 {
            return None;
        }
        if flag {
            let mut rewritten = PacketWrapper::new(&clientbound::play::EXPLODE, source_sound);
            if !sound::rewrite_holder(&mut rewritten, target, ids).ok()? {
                return None;
            }
            out.extend_from_slice(&rewritten.finish().ok()??.payload);
        } else {
            out.write_var_int(&VarInt(0)).ok()?;
            out.write_string("intentionally_empty").ok()?;
            out.write_bool(false).ok()?;
        }
        out.write_var_int(&VarInt(block_particle_pool)).ok()?;
    }
    Some(out)
}

fn explode(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    // Pumpkin's CExplosion writer already omits the 26.3 flag for every older
    // client version. Keep its native target layout untouched in that path.
    if ctx.layout < V::V_26_3 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let out = rewrite_explosion_payload(wrapper.remaining(), connection.version)
        .ok_or(TranslateError::Unsupported("26.3 explosion"))?;
    wrapper.replace_remaining(out);
    Ok(())
}

fn open_sign_editor(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    if connection.version < V::V_1_20 {
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.passthrough(&I64)?;
    let side = wrapper.read(&VAR_INT)?.0;
    wrapper.write(&BOOL, &(side == 1))?;
    wrapper.passthrough_all();
    Ok(())
}

fn sign_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    // Older sign-update packets gain the side boolean in the 1.20 step before
    // reaching this boundary, so the input here is always the 26.2 shape.
    wrapper.passthrough(&I64)?;
    let front_text = wrapper.read(&BOOL)?;
    for _ in 0..4 {
        wrapper.passthrough(&STRING)?;
    }
    wrapper.write(&VAR_INT, &VarInt(if front_text { 1 } else { 0 }))?;
    wrapper.passthrough_all();
    Ok(())
}

fn open_screen(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let container_id = wrapper.passthrough(&VAR_INT)?.0;
    let menu_type = wrapper.passthrough(&VAR_INT)?.0;
    let brewing_stand_menu = MappingData::get()
        .composed(ctx.layout)
        .menus
        .map(BREWING_STAND_MENU_TYPE as u32)
        .and_then(|id| i32::try_from(id).ok());
    if connection.get::<ScreenStorage>().is_none() {
        connection.put(ScreenStorage::default());
    }
    connection
        .get_mut::<ScreenStorage>()
        .expect("screen storage was just inserted")
        .brewing_stand_container_id =
        (Some(menu_type) == brewing_stand_menu).then_some(container_id);
    wrapper.passthrough_all();
    Ok(())
}

fn container_set_data(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let container_id = if connection.version >= V::V_1_21_2 {
        wrapper.passthrough(&VAR_INT)?.0
    } else {
        i32::from(wrapper.passthrough(&U8)?)
    };
    let slot = wrapper.passthrough(&I16)?;
    if slot >= 2
        && connection
            .get::<ScreenStorage>()
            .and_then(|storage| storage.brewing_stand_container_id)
            == Some(container_id)
    {
        wrapper.cancel();
        return Ok(());
    }
    wrapper.passthrough(&I16)?; // value
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Protocol;
    use pumpkin_protocol::{ClientPacket, IdOr, java::client::play::CExplosion};
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn pottery_side_data_becomes_the_legacy_sherd_list() {
        let mut decorations = NbtCompound::new();
        for (side, item) in [
            ("back", "minecraft:brick"),
            ("left", "minecraft:angler_pottery_sherd"),
            ("front", "minecraft:prize_pottery_sherd"),
        ] {
            let mut entry = NbtCompound::new();
            entry.put_string("id", item.to_string());
            decorations.put_compound(side, entry);
        }
        let mut root = NbtCompound::new();
        root.put_compound("sherds", decorations);
        let NbtTag::Compound(out) = downgrade_pot_sherds(Some(NbtTag::Compound(root))).unwrap()
        else {
            panic!("compound");
        };
        assert_eq!(
            out.get_list("sherds").unwrap(),
            &vec![
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:angler_pottery_sherd".into()),
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:prize_pottery_sherd".into()),
            ]
        );
    }

    #[test]
    fn chunk_block_entities_keep_the_light_tail_while_downgrading_pots() {
        let version = V::V_26_2;
        let mut decorations = NbtCompound::new();
        let mut side = NbtCompound::new();
        side.put_string("id", "minecraft:angler_pottery_sherd".to_string());
        decorations.put_compound("left", side);
        let mut block_entity = NbtCompound::new();
        block_entity.put_compound("sherds", decorations);

        let mut payload = Vec::new();
        payload.write_i32_be(2).unwrap();
        payload.write_i32_be(-3).unwrap();
        NbtT::for_version(version)
            .write(&mut payload, &None)
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // empty section data
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // one block entity
        payload.push(0x21);
        payload.write_i16_be(70).unwrap();
        VAR_INT.write(&mut payload, &VarInt(9)).unwrap();
        NbtT::for_version(version)
            .write(&mut payload, &Some(NbtTag::Compound(block_entity)))
            .unwrap();
        payload.extend_from_slice(&[0xaa, 0xbb]); // light tail

        let out = rewrite_chunk_block_entities(&payload, version).unwrap();
        let mut cursor = out.as_slice();
        assert_eq!(cursor.get_i32_be().unwrap(), 2);
        assert_eq!(cursor.get_i32_be().unwrap(), -3);
        NbtT::for_version(version).read(&mut cursor).unwrap();
        assert_eq!(cursor.get_var_int().unwrap().0, 0);
        assert_eq!(cursor.get_var_int().unwrap().0, 1);
        assert_eq!(cursor.get_u8().unwrap(), 0x21);
        assert_eq!(cursor.get_i16_be().unwrap(), 70);
        assert_eq!(cursor.get_var_int().unwrap().0, 9);
        let NbtTag::Compound(block_entity) = NbtT::for_version(version)
            .read(&mut cursor)
            .unwrap()
            .unwrap()
        else {
            panic!("block entity NBT is a compound");
        };
        assert_eq!(block_entity.get_list("sherds").unwrap().len(), 4);
        assert_eq!(cursor, [0xaa, 0xbb]);
    }

    #[test]
    fn bitset_byte_array_is_converted_to_a_long_array() {
        let mut input = vec![2]; // two bytes
        input.extend_from_slice(&[0x01, 0x80]);
        let mut cursor = input.as_slice();
        let mut output = Vec::new();
        bit_set_to_long_array(&mut cursor, &mut output).unwrap();
        assert!(cursor.is_empty());
        let mut decoded = output.as_slice();
        assert_eq!(decoded.get_var_int().unwrap().0, 1);
        assert_eq!(
            decoded.get_i64_be().unwrap(),
            i64::from_le_bytes([1, 128, 0, 0, 0, 0, 0, 0])
        );
        assert!(decoded.is_empty());
    }

    #[test]
    fn an_explosion_with_play_sound_false_gets_a_silent_legacy_holder() {
        let target = V::V_26_2;
        let ids = crate::api::MappingData::get().composed(target);
        let sound_id = (0..1000u32)
            .find(|id| ids.sounds.map(*id).is_some())
            .expect("a sound mapping is available");
        let explosion = CExplosion::new(
            Vector3::new(1.0, 2.0, 3.0),
            4.0,
            0,
            None,
            VarInt(pumpkin_data::particle::Particle::ExplosionEmitter as i32),
            IdOr::Id(u16::try_from(sound_id).unwrap()),
        );
        let mut payload = Vec::new();
        explosion
            .write_packet_data(&mut payload, &V::V_26_3)
            .unwrap();
        *payload.last_mut().unwrap() = 0; // 26.3 playSound=false

        let translated = rewrite_explosion_payload(&payload, target).unwrap();
        assert!(
            translated
                .windows("intentionally_empty".len())
                .any(|bytes| bytes == b"intentionally_empty")
        );
        assert_eq!(
            translated.last(),
            Some(&0),
            "target payload has no playSound flag"
        );
    }
}
