use std::collections::HashMap;

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::types::{BOOL, F32, F64, I16, I32, STRING, U8, UUID, VAR_INT, WireType};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::{clientbound, serverbound};
use pumpkin_util::text::{TextComponent, TextContent};

const V1_14: JavaMinecraftVersion = JavaMinecraftVersion::V_1_14;
const V1_13_2: JavaMinecraftVersion = JavaMinecraftVersion::V_1_13_2;
const RELATIVE_MOVE_FACTOR: f64 = 4096.0;

pub struct Protocol1_14To1_13_2;

impl Protocol for Protocol1_14To1_13_2 {
    fn step(&self) -> Step {
        Step {
            from: V1_14,
            to: V1_13_2,
        }
    }

    fn register(&self, reg: &mut Registry) {
        // These packets are only hints for the 1.14 chunk cache.
        reg.cancel_clientbound(&clientbound::play::SET_CHUNK_CACHE_CENTER);
        reg.cancel_clientbound(&clientbound::play::SET_CHUNK_CACHE_RADIUS);

        // Core writes the 1.14 locked bit on this packet; 1.13.2 has only the
        // difficulty byte. This packet's core floor is newer than the client.
        reg.clientbound_layout(&clientbound::play::CHANGE_DIFFICULTY, difficulty);
        reg.clientbound_layout(&clientbound::play::OPEN_SCREEN, open_screen);
        reg.clientbound_layout(&clientbound::play::BLOCK_UPDATE, block_update);
        reg.clientbound_layout(&clientbound::play::BLOCK_EVENT, block_event);
        reg.clientbound_layout(&clientbound::play::LEVEL_EVENT, level_event);
        reg.clientbound(&clientbound::play::EXPLODE, explosion);
        reg.clientbound(&clientbound::play::UPDATE_TAGS, strip_entity_tags);
        reg.clientbound(&clientbound::play::COMMANDS, commands);

        // Track positions for ViaBackwards' entity-sound -> positional-sound
        // fallback. These packets already have the client layout at this point.
        reg.clientbound(&clientbound::play::ADD_ENTITY, track_spawn_entity);
        reg.clientbound(&clientbound::play::SPAWN_PLAYER, track_spawn_player);
        reg.clientbound(&clientbound::play::SPAWN_PAINTING, track_spawn_painting);
        reg.clientbound(&clientbound::play::TELEPORT_ENTITY, track_teleport);
        reg.clientbound(&clientbound::play::MOVE_ENTITY_POS, track_relative_move);
        reg.clientbound(&clientbound::play::MOVE_ENTITY_POS_ROT, track_relative_move);
        reg.clientbound(&clientbound::play::REMOVE_ENTITIES, forget_entities);
        reg.clientbound(&clientbound::play::LOGIN, clear_entity_positions);
        reg.clientbound(&clientbound::play::RESPAWN, clear_entity_positions);
        reg.clientbound(&clientbound::play::SOUND_ENTITY, entity_sound);

        // 1.13.2 sends position, face, hand; 1.14 expects hand, position,
        // face, and an `inside block` flag.
        reg.serverbound(&serverbound::play::USE_ITEM_ON, use_item_on);
        // 1.14 added four recipe-book toggles for blast furnace and smoker.
        reg.serverbound(
            &serverbound::play::RECIPE_BOOK_CHANGE_SETTINGS,
            recipe_book_settings,
        );
    }
}

fn difficulty(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&U8)?;
    wrapper.read(&BOOL)?;
    Ok(())
}

/// Rewrites 1.14's numeric menu to the legacy 1.13.2 inventory name and slot
/// count. Menus without a safe 1.13 equivalent are canceled, as in ViaBackwards.
fn open_screen(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let window = wrapper.read(&VAR_INT)?.0;
    let menu = wrapper.read(&VAR_INT)?.0;
    let Some((name, slots)) = legacy_menu(menu) else {
        wrapper.consume_remaining();
        wrapper.cancel();
        return Ok(());
    };
    let mut title = wrapper.read(&crate::api::types::TextComponentT::for_version(ctx.layout))?;
    if let Some((key, replacement)) = legacy_title(menu)
        && matches!(&*title.0.content, TextContent::Translate { translate, .. } if &**translate == key)
    {
        title = TextComponent::text(replacement);
    }
    wrapper.write(&U8, &(window as u8))?;
    wrapper.write(&STRING, &name.into())?;
    wrapper.write(
        &crate::api::types::TextComponentT::for_version(V1_13_2),
        &title,
    )?;
    wrapper.write(&U8, &slots)?;
    Ok(())
}

fn legacy_menu(id: i32) -> Option<(&'static str, u8)> {
    let value = match id {
        0..=5 => ("minecraft:container", ((id + 1) * 9) as u8),
        6 => ("minecraft:dropper", 9),
        7 => ("minecraft:anvil", 0),
        8 => ("minecraft:beacon", 1),
        9 | 13 | 14 | 20 => ("minecraft:furnace", 3),
        10 => ("minecraft:brewing_stand", 5),
        11 => ("minecraft:crafting_table", 0),
        12 => ("minecraft:enchanting_table", 0),
        15 => ("minecraft:hopper", 5),
        18 => ("minecraft:villager", 0),
        19 => ("minecraft:shulker_box", 27),
        21 => ("minecraft:anvil", 0),
        _ => return None,
    };
    Some(value)
}

fn legacy_title(id: i32) -> Option<(&'static str, &'static str)> {
    match id {
        2 => Some(("container.barrel", "Barrel")),
        9 => Some(("container.blast_furnace", "Blast Furnace")),
        14 => Some(("container.grindstone", "Grindstone")),
        20 => Some(("container.smoker", "Smoker")),
        21 => Some(("container.cartography_table", "Cartography Table")),
        _ => None,
    }
}

fn block_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&crate::api::types::BLOCK_POS)?;
    let state = wrapper.read(&VAR_INT)?.0;
    let mapped = u32::try_from(state)
        .ok()
        .and_then(|id| ctx.mappings.blockstates.map(id))
        .unwrap_or(0);
    wrapper.write(&VAR_INT, &VarInt(i32::try_from(mapped).unwrap_or(0)))?;
    Ok(())
}

fn block_event(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&crate::api::types::BLOCK_POS)?;
    wrapper.passthrough(&U8)?;
    wrapper.passthrough(&U8)?;
    let block = wrapper.read(&VAR_INT)?.0;
    let mapped = u32::try_from(block)
        .ok()
        .and_then(|id| ctx.mappings.blocks.map(id));
    let Some(mapped) = mapped else {
        wrapper.cancel();
        wrapper.consume_remaining();
        return Ok(());
    };
    wrapper.write(&VAR_INT, &VarInt(i32::try_from(mapped).unwrap_or(0)))?;
    Ok(())
}

fn level_event(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let event = wrapper.passthrough(&I32)?;
    wrapper.passthrough(&crate::api::types::BLOCK_POS)?;
    let data = wrapper.read(&I32)?;
    let mapped = match event {
        1010 => u32::try_from(data)
            .ok()
            .and_then(|id| ctx.mappings.items.map(id))
            .and_then(|id| i32::try_from(id).ok())
            .unwrap_or(-1),
        2001 => u32::try_from(data)
            .ok()
            .and_then(|id| ctx.mappings.blockstates.map(id))
            .and_then(|id| i32::try_from(id).ok())
            .unwrap_or(0),
        _ => data,
    };
    wrapper.write(&I32, &mapped)?;
    Ok(())
}

fn explosion(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    for _ in 0..3 {
        let coordinate = wrapper.read(&F32)?;
        let adjusted = if coordinate < 0.0 {
            coordinate.floor()
        } else {
            coordinate
        };
        wrapper.write(&F32, &adjusted)?;
    }
    wrapper.passthrough(&F32)?;
    wrapper.passthrough_all();
    Ok(())
}

/// 1.14 sends block, item, fluid, and entity tag lists. Entity tags did not
/// exist in 1.13.2; keep the first three lists and consume the fourth.
fn strip_entity_tags(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let input = wrapper.remaining();
    let mut cursor = input;
    let mut output = Vec::with_capacity(input.len());
    for index in 0..4 {
        let count_wire = cursor.get_var_int()?;
        let count = usize::try_from(count_wire.0)
            .map_err(|_| TranslateError::Unsupported("negative tag count"))?;
        if index < 3 {
            output.write_var_int(&count_wire)?;
        }
        for _ in 0..count {
            let name = cursor.get_str()?;
            let ids_len_wire = cursor.get_var_int()?;
            let ids_len = usize::try_from(ids_len_wire.0)
                .map_err(|_| TranslateError::Unsupported("negative tag member count"))?;
            if index < 3 {
                output.write_string(&name)?;
                output.write_var_int(&ids_len_wire)?;
            }
            for _ in 0..ids_len {
                let id = cursor.get_var_int()?;
                if index < 3 {
                    output.write_var_int(&id)?;
                }
            }
        }
    }
    if !cursor.is_empty() {
        return Err(TranslateError::TrailingBytes(cursor.len()));
    }
    wrapper.replace_remaining(output);
    Ok(())
}

fn commands(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let count_wire = wrapper.read(&VAR_INT)?;
    let count = usize::try_from(count_wire.0)
        .map_err(|_| TranslateError::Unsupported("negative command node count"))?;
    wrapper.write(&VAR_INT, &count_wire)?;
    for _ in 0..count {
        let flags = wrapper.passthrough(&U8)?;
        let children_wire = wrapper.read(&VAR_INT)?;
        let children = usize::try_from(children_wire.0)
            .map_err(|_| TranslateError::Unsupported("negative command child count"))?;
        wrapper.write(&VAR_INT, &children_wire)?;
        for _ in 0..children {
            wrapper.passthrough(&VAR_INT)?;
        }
        if flags & 0x08 != 0 {
            wrapper.passthrough(&VAR_INT)?;
        }
        let kind = flags & 0x03;
        if kind == 1 || kind == 2 {
            wrapper.passthrough(&STRING)?;
        }
        if kind == 2 {
            let parser: String = wrapper.read(&STRING)?.into();
            match parser.as_str() {
                "minecraft:nbt_compound_tag" => wrapper.write(&STRING, &"minecraft:nbt".into())?,
                "minecraft:nbt_tag" => {
                    wrapper.write(&STRING, &"brigadier:string".into())?;
                    wrapper.write(&VAR_INT, &VarInt(2))?;
                }
                "minecraft:time" => {
                    wrapper.write(&STRING, &"brigadier:integer".into())?;
                    wrapper.write(&U8, &1)?;
                    wrapper.write(&I32, &0)?;
                }
                _ => {
                    wrapper.write(&STRING, &parser.clone().into())?;
                    copy_command_properties(wrapper, &parser)?;
                }
            }
        }
        if flags & 0x10 != 0 {
            wrapper.passthrough(&STRING)?;
        }
    }
    wrapper.passthrough(&VAR_INT)?;
    Ok(())
}

fn copy_command_properties(
    wrapper: &mut PacketWrapper,
    parser: &str,
) -> Result<(), TranslateError> {
    match parser {
        "brigadier:double" => copy_number_properties(wrapper, &F64)?,
        "brigadier:float" => copy_number_properties(wrapper, &F32)?,
        "brigadier:integer" => copy_number_properties(wrapper, &I32)?,
        "brigadier:long" => copy_number_properties(wrapper, &crate::api::types::I64)?,
        "brigadier:string" => {
            wrapper.passthrough(&VAR_INT)?;
        }
        "minecraft:entity" | "minecraft:score_holder" => {
            wrapper.passthrough(&U8)?;
        }
        // The remaining 1.14 argument parsers have no payload properties.
        _ => {}
    }
    Ok(())
}

fn copy_number_properties<T: WireType>(
    wrapper: &mut PacketWrapper,
    number: &T,
) -> Result<(), TranslateError> {
    let flags = wrapper.passthrough(&U8)?;
    if flags & 1 != 0 {
        wrapper.passthrough(number)?;
    }
    if flags & 2 != 0 {
        wrapper.passthrough(number)?;
    }
    Ok(())
}

#[derive(Default)]
struct EntityPositions(HashMap<i32, [f64; 3]>);

fn entity_positions(connection: &mut UserConnection) -> &mut EntityPositions {
    if connection.get::<EntityPositions>().is_none() {
        connection.put(EntityPositions::default());
    }
    connection
        .get_mut::<EntityPositions>()
        .expect("inserted above")
}

fn track_spawn_entity(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let spawn = pumpkin_protocol::java::client::play::CSpawnEntity::read_packet_data(
        wrapper.remaining(),
        &ctx.layout,
    )?;
    entity_positions(connection).0.insert(
        spawn.entity_id.0,
        [spawn.position.x, spawn.position.y, spawn.position.z],
    );
    wrapper.passthrough_all();
    Ok(())
}

fn track_spawn_player(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < JavaMinecraftVersion::V_1_9 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let id = wrapper.passthrough(&VAR_INT)?.0;
    wrapper.passthrough(&UUID)?;
    let position = [
        wrapper.passthrough(&F64)?,
        wrapper.passthrough(&F64)?,
        wrapper.passthrough(&F64)?,
    ];
    entity_positions(connection).0.insert(id, position);
    wrapper.passthrough_all();
    Ok(())
}

fn track_spawn_painting(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_13_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let id = wrapper.passthrough(&VAR_INT)?.0;
    wrapper.passthrough(&UUID)?;
    wrapper.passthrough(&VAR_INT)?;
    let position = wrapper.passthrough(&crate::api::types::BLOCK_POS)?;
    entity_positions(connection).0.insert(
        id,
        [
            f64::from(position.0.x),
            f64::from(position.0.y),
            f64::from(position.0.z),
        ],
    );
    wrapper.passthrough_all();
    Ok(())
}

fn track_teleport(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let id = wrapper.passthrough(&VAR_INT)?.0;
    let position = [
        wrapper.passthrough(&F64)?,
        wrapper.passthrough(&F64)?,
        wrapper.passthrough(&F64)?,
    ];
    entity_positions(connection).0.insert(id, position);
    wrapper.passthrough_all();
    Ok(())
}

fn track_relative_move(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_13_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let id = wrapper.passthrough(&VAR_INT)?.0;
    let delta = [
        f64::from(wrapper.passthrough(&I16)?),
        f64::from(wrapper.passthrough(&I16)?),
        f64::from(wrapper.passthrough(&I16)?),
    ];
    let positions = entity_positions(connection);
    if let Some(position) = positions.0.get_mut(&id) {
        for axis in 0..3 {
            position[axis] += delta[axis] / RELATIVE_MOVE_FACTOR;
        }
    }
    wrapper.passthrough_all();
    Ok(())
}

fn forget_entities(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_13_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let count_wire = wrapper.read(&VAR_INT)?;
    let count = usize::try_from(count_wire.0)
        .map_err(|_| TranslateError::Unsupported("negative removed-entity count"))?;
    wrapper.write(&VAR_INT, &count_wire)?;
    for _ in 0..count {
        let id = wrapper.passthrough(&VAR_INT)?.0;
        entity_positions(connection).0.remove(&id);
    }
    wrapper.passthrough_all();
    Ok(())
}

fn clear_entity_positions(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    entity_positions(connection).0.clear();
    wrapper.passthrough_all();
    Ok(())
}

fn entity_sound(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_13_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let sound = wrapper.read(&VAR_INT)?;
    let category = wrapper.read(&VAR_INT)?;
    let entity = wrapper.read(&VAR_INT)?.0;
    let volume = wrapper.read(&F32)?;
    let pitch = wrapper.read(&F32)?;
    wrapper.consume_remaining();

    let Some(position) = connection
        .get::<EntityPositions>()
        .and_then(|positions| positions.0.get(&entity))
        .copied()
    else {
        wrapper.cancel();
        return Ok(());
    };

    let mut payload = Vec::with_capacity(25);
    VAR_INT.write(&mut payload, &sound)?;
    VAR_INT.write(&mut payload, &category)?;
    for coordinate in position {
        I32.write(&mut payload, &((coordinate * 8.0) as i32))?;
    }
    F32.write(&mut payload, &volume)?;
    F32.write(&mut payload, &pitch)?;
    wrapper.send_extra(&clientbound::play::SOUND, payload);
    wrapper.cancel();
    Ok(())
}

fn use_item_on(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_13_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let position = wrapper.read(&crate::api::types::BLOCK_POS)?;
    let face = wrapper.read(&VAR_INT)?;
    let hand = wrapper.read(&VAR_INT)?;
    let x = wrapper.read(&F32)?;
    let y = wrapper.read(&F32)?;
    let z = wrapper.read(&F32)?;

    wrapper.write(&VAR_INT, &hand)?;
    wrapper.write(&crate::api::types::BLOCK_POS, &position)?;
    wrapper.write(&VAR_INT, &face)?;
    wrapper.write(&F32, &x)?;
    wrapper.write(&F32, &y)?;
    wrapper.write(&F32, &z)?;
    wrapper.write(&BOOL, &false)?;
    Ok(())
}

fn recipe_book_settings(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < V1_13_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let action = wrapper.passthrough(&VAR_INT)?.0;
    match action {
        0 => {
            wrapper.passthrough(&STRING)?;
        }
        1 => {
            for _ in 0..4 {
                wrapper.passthrough(&BOOL)?;
            }
            for _ in 0..4 {
                wrapper.write(&BOOL, &false)?;
            }
        }
        _ => return Err(TranslateError::Unsupported("recipe-book action")),
    }
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::api::types::WireType;
    use pumpkin_protocol::ser::NetworkWriteExt;

    fn context() -> Ctx<'static> {
        Ctx {
            step: Protocol1_14To1_13_2.step(),
            mappings: MappingData::get().step(V1_14),
            layout: V1_14,
        }
    }

    #[test]
    fn use_item_on_reorders_position_face_and_hand_and_adds_inside_flag() {
        let mut input = Vec::new();
        crate::api::types::BLOCK_POS
            .write(
                &mut input,
                &pumpkin_util::math::position::BlockPos::new(4, 70, -9),
            )
            .unwrap();
        VAR_INT.write(&mut input, &VarInt(5)).unwrap();
        VAR_INT.write(&mut input, &VarInt(1)).unwrap();
        for value in [0.25f32, 0.5, 0.75] {
            F32.write(&mut input, &value).unwrap();
        }

        let mut wrapper = PacketWrapper::new(&serverbound::play::USE_ITEM_ON, &input);
        use_item_on(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(
            crate::api::types::BLOCK_POS.read(&mut read).unwrap(),
            pumpkin_util::math::position::BlockPos::new(4, 70, -9)
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 5);
        assert_eq!(F32.read(&mut read).unwrap(), 0.25);
        assert_eq!(F32.read(&mut read).unwrap(), 0.5);
        assert_eq!(F32.read(&mut read).unwrap(), 0.75);
        assert!(!BOOL.read(&mut read).unwrap());
        assert!(read.is_empty());
    }

    #[test]
    fn recipe_book_settings_add_four_false_1_14_toggles() {
        let input = [1, 1, 0, 1, 0];
        let mut wrapper =
            PacketWrapper::new(&serverbound::play::RECIPE_BOOK_CHANGE_SETTINGS, &input);
        recipe_book_settings(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        assert_eq!(output, [1, 1, 0, 1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_1_14_locked_difficulty_bit_is_removed() {
        let mut wrapper = PacketWrapper::new(&clientbound::play::CHANGE_DIFFICULTY, &[3, 1]);
        difficulty(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        assert_eq!(wrapper.finish().unwrap().unwrap().payload, [3]);
    }

    #[test]
    fn supported_menus_become_legacy_names_and_unsupported_menus_cancel() {
        let mut payload = vec![7, 2]; // window 7, 1.14 menu type 2
        crate::api::types::TextComponentT::for_version(V1_14)
            .write(
                &mut payload,
                &pumpkin_util::text::TextComponent::text("Barrel"),
            )
            .unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::OPEN_SCREEN, &payload);
        open_screen(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(U8.read(&mut read).unwrap(), 7);
        assert_eq!(&*STRING.read(&mut read).unwrap(), "minecraft:container");
        crate::api::types::TextComponentT::for_version(V1_13_2)
            .read(&mut read)
            .unwrap();
        assert_eq!(U8.read(&mut read).unwrap(), 27);
        assert!(read.is_empty());

        let mut unsupported = vec![7, 16];
        crate::api::types::TextComponentT::for_version(V1_14)
            .write(
                &mut unsupported,
                &pumpkin_util::text::TextComponent::text("Loom"),
            )
            .unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::OPEN_SCREEN, &unsupported);
        open_screen(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        assert!(wrapper.finish().unwrap().is_none());
    }

    #[test]
    fn explosion_negative_coordinates_use_floor() {
        let mut payload = Vec::new();
        for value in [-0.25f32, -1.75, 1.75, 2.0] {
            F32.write(&mut payload, &value).unwrap();
        }
        payload.extend_from_slice(&[0, 0, 0]);
        let mut wrapper = PacketWrapper::new(&clientbound::play::EXPLODE, &payload);
        explosion(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(F32.read(&mut read).unwrap(), -1.0);
        assert_eq!(F32.read(&mut read).unwrap(), -2.0);
        assert_eq!(F32.read(&mut read).unwrap(), 1.75);
        assert_eq!(F32.read(&mut read).unwrap(), 2.0);
        assert_eq!(read, [0, 0, 0]);
    }

    #[test]
    fn command_parser_aliases_keep_their_1_14_wire_meaning() {
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(2)).unwrap();
        payload.write_u8(0).unwrap();
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.write_var_int(&VarInt(1)).unwrap();
        payload.write_u8(2 | 4).unwrap();
        payload.write_var_int(&VarInt(0)).unwrap();
        payload.write_string("time").unwrap();
        payload.write_string("minecraft:time").unwrap();
        payload.write_var_int(&VarInt(0)).unwrap();

        let mut wrapper = PacketWrapper::new(&clientbound::play::COMMANDS, &payload);
        commands(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2);
        U8.read(&mut read).unwrap();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        VAR_INT.read(&mut read).unwrap();
        let flags = U8.read(&mut read).unwrap();
        VAR_INT.read(&mut read).unwrap();
        assert_eq!(&*STRING.read(&mut read).unwrap(), "time");
        assert_eq!(&*STRING.read(&mut read).unwrap(), "brigadier:integer");
        assert_eq!(U8.read(&mut read).unwrap(), 1);
        assert_eq!(I32.read(&mut read).unwrap(), 0);
        assert_ne!(flags & 0x04, 0);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0);
        assert!(read.is_empty());
    }

    #[test]
    fn tag_rewrite_drops_only_entity_tags() {
        fn write_list(out: &mut Vec<u8>, name: &str) {
            out.write_var_int(&VarInt(1)).unwrap();
            out.write_string(name).unwrap();
            out.write_var_int(&VarInt(1)).unwrap();
            out.write_var_int(&VarInt(7)).unwrap();
        }
        let mut payload = Vec::new();
        for name in ["blocks", "items", "fluids", "entities"] {
            write_list(&mut payload, name);
        }
        let mut wrapper = PacketWrapper::new(&clientbound::play::UPDATE_TAGS, &payload);
        strip_entity_tags(
            &mut wrapper,
            &mut UserConnection::new(0, V1_13_2),
            &context(),
        )
        .unwrap();
        let output = wrapper.finish().unwrap().unwrap().payload;
        let mut read = output.as_slice();
        for expected in ["blocks", "items", "fluids"] {
            assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
            assert_eq!(&*STRING.read(&mut read).unwrap(), expected);
            assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
            assert_eq!(VAR_INT.read(&mut read).unwrap().0, 7);
        }
        assert!(read.is_empty());
    }
}
