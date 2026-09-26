use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion as V;

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

    crate::packet::chunk_remap::copy_heightmaps(&mut cursor, &mut out, version)?;
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
        .ok_or(TranslateError::Unsupported("chunk payload layout"))?;
    wrapper.replace_remaining(translated);
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
        // Since 1.21.5, chunk heightmaps are a counted list of registry IDs
        // and packed longs, not an NBT compound.
        VAR_INT.write(&mut payload, &VarInt(3)).unwrap(); // Pumpkin's three heightmaps
        for (index, seed) in [(1, 10_i64), (4, 20), (5, 30)] {
            VAR_INT.write(&mut payload, &VarInt(index)).unwrap();
            VAR_INT.write(&mut payload, &VarInt(37)).unwrap(); // packed long count
            for offset in 0..37 {
                payload.write_i64_be(seed + offset).unwrap();
            }
        }
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
        assert_eq!(cursor.get_var_int().unwrap().0, 3);
        for (index, seed) in [(1, 10_i64), (4, 20), (5, 30)] {
            assert_eq!(cursor.get_var_int().unwrap().0, index);
            assert_eq!(cursor.get_var_int().unwrap().0, 37);
            for offset in 0..37 {
                assert_eq!(cursor.get_i64_be().unwrap(), seed + offset);
            }
        }
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
}
