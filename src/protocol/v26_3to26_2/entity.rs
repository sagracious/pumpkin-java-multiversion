use pumpkin_data::entity::EntityType;
use pumpkin_protocol::ClientPacket;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CSpawnEntity;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::entity_data::{EntityDataEntry, EntityDataListT, MetaValue};
use crate::api::types::{BOOL, F32, F64, I8, I16, U8, VAR_INT};
use crate::api::{Ctx, MappingData, PacketWrapper, Registry, TranslateError, UserConnection};
use crate::data::entity_data_types::meta_data_type_id_for_name;
use crate::packet::mappings::{clientbound, serverbound};

#[derive(Default)]
struct StepStorage {
    pending_teleport_id: Option<i32>,
}

pub(super) fn register(reg: &mut Registry) {
    reg.clientbound(&clientbound::play::MOVE_ENTITY_POS, move_position);
    reg.clientbound(
        &clientbound::play::MOVE_ENTITY_POS_ROT,
        move_position_rotation,
    );
    reg.clientbound(&clientbound::play::MOVE_ENTITY_ROT, move_rotation);
    reg.clientbound(&clientbound::play::ENTITY_POSITION_SYNC, position_sync);
    reg.clientbound(&clientbound::play::SWING_ANIMATION, swing_animation);
    reg.clientbound(&clientbound::play::ADD_ENTITY, cushion_spawn);

    reg.serverbound(&serverbound::play::PLAYER_ACTION, player_action);
    reg.serverbound(&serverbound::play::SPECTATE_ENTITY, spectate_entity);
    reg.serverbound(
        &serverbound::play::ACCEPT_TELEPORTATION,
        accept_teleportation,
    );
    reg.serverbound(
        &serverbound::play::MOVE_PLAYER_POS_ROT,
        complete_teleportation,
    );
}

/// 26.3 packs a step count and the on-ground flag into one property integer,
/// and may include several tick-spaced deltas. 26.2 reads their summed delta.
fn move_position(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let on_ground = move_position_body(wrapper)?;
    wrapper.write(&BOOL, &on_ground)?;
    wrapper.passthrough_all();
    Ok(())
}

fn move_position_body(wrapper: &mut PacketWrapper) -> Result<bool, TranslateError> {
    let properties = wrapper.read(&VAR_INT)?.0;
    let steps = (properties >> 1).max(0);
    if steps == 0 {
        for _ in 0..3 {
            wrapper.passthrough(&I16)?;
        }
        return Ok(properties & 1 != 0);
    }
    let mut delta = [0i32; 3];
    for _ in 0..steps {
        wrapper.passthrough(&VAR_INT)?; // tick delay
        for value in &mut delta {
            *value = value.wrapping_add(i32::from(wrapper.read(&I16)?));
        }
    }
    for value in delta {
        wrapper.write(&I16, &(value as i16))?;
    }
    Ok(properties & 1 != 0)
}

fn move_position_rotation(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let on_ground = move_position_body(wrapper)?;
    wrapper.passthrough(&I8)?; // Y rotation
    wrapper.passthrough(&I8)?; // X rotation
    wrapper.write(&BOOL, &on_ground)?;
    wrapper.passthrough_all();
    Ok(())
}

fn move_rotation(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let on_ground = wrapper.read(&BOOL)?;
    wrapper.passthrough(&I8)?; // Y rotation
    wrapper.passthrough(&I8)?; // X rotation
    wrapper.write(&BOOL, &on_ground)?;
    wrapper.passthrough_all();
    Ok(())
}

/// A position path is reduced to its final point for clients predating the
/// new path encoding.  The final segment supplies a best-effort velocity.
fn position_sync(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?; // entity id
    let path_type = wrapper.read(&VAR_INT)?.0;
    if path_type == 0 {
        for _ in 0..3 {
            wrapper.passthrough(&F64)?;
        }
        for _ in 0..3 {
            wrapper.write(&F64, &0.0)?;
        }
        wrapper.passthrough_all();
        return Ok(());
    }

    let steps = wrapper.read(&VAR_INT)?.0;
    if !(0..=1024).contains(&steps) {
        return Err(TranslateError::Unsupported("entity position path length"));
    }
    let mut previous = [0.0f64; 3];
    let mut current = [0.0f64; 3];
    let mut last_tick_offset = 0;
    for _ in 0..steps {
        previous = current;
        for value in &mut current {
            *value = wrapper.read(&F64)?;
        }
        last_tick_offset = wrapper.read(&VAR_INT)?.0;
    }
    for value in current {
        wrapper.write(&F64, &value)?;
    }
    for axis in 0..3 {
        let velocity = if steps > 1 && last_tick_offset > 0 {
            (current[axis] - previous[axis]) / f64::from(last_tick_offset)
        } else {
            0.0
        };
        wrapper.write(&F64, &velocity)?;
    }
    wrapper.passthrough_all();
    Ok(())
}

fn swing_animation(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?; // entity id
    let hand = wrapper.read(&VAR_INT)?.0;
    let animation_type = wrapper.read(&VAR_INT)?.0;
    wrapper.passthrough(&VAR_INT)?; // duration
    if animation_type == 0 {
        wrapper.cancel();
        return Ok(());
    }
    wrapper.set_packet(&clientbound::play::ANIMATE);
    wrapper.write(&U8, &if hand == 0 { 0 } else { 3 })?;
    wrapper.passthrough_all();
    Ok(())
}

fn cushion_spawn(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let mut cursor = wrapper.remaining();
    let entity_id = cursor.get_var_int()?.0;
    if connection.entity_tracker.entity_type(entity_id) != Some(EntityType::CUSHION.id) {
        wrapper.passthrough_all();
        return Ok(());
    }

    let spawn = CSpawnEntity::read_packet_data(wrapper.remaining(), &ctx.layout)?;
    let carpet_state = pumpkin_data::Block::WHITE_CARPET.default_state.id.as_u16();
    let mapped_state = MappingData::get()
        .composed(ctx.layout)
        .blockstates
        .map(u32::from(carpet_state))
        .and_then(|id| i32::try_from(id).ok())
        .ok_or(TranslateError::Unsupported("white carpet block state"))?;
    let mut payload = Vec::with_capacity(wrapper.remaining().len());
    CSpawnEntity {
        data: VarInt(mapped_state),
        ..spawn
    }
    .write_packet_data(&mut payload, &ctx.layout)?;
    wrapper.replace_remaining(payload);

    let serializer = meta_data_type_id_for_name("boolean", ctx.layout)
        .ok_or(TranslateError::Unsupported("boolean metadata serializer"))?;
    let entries = vec![EntityDataEntry {
        index: 5,
        serializer,
        value: MetaValue::Raw(vec![1]), // no gravity
    }];
    let mut metadata = Vec::new();
    EntityDataListT::for_version(ctx.layout).write(&mut metadata, &entries)?;
    let mut payload = Vec::with_capacity(metadata.len() + 5);
    VAR_INT.write(&mut payload, &VarInt(entity_id))?;
    payload.extend_from_slice(&metadata);
    wrapper.send_extra(&clientbound::play::SET_ENTITY_DATA, payload);
    Ok(())
}

fn player_action(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let action = wrapper.read(&VAR_INT)?.0;
    wrapper.write(
        &VAR_INT,
        &VarInt(if action >= 1 { action + 1 } else { action }),
    )?;
    wrapper.passthrough_all();
    Ok(())
}

fn spectate_entity(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.set_packet(&serverbound::play::SPECTATOR_ACTION);
    wrapper.passthrough_all();
    Ok(())
}

fn accept_teleportation(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let id = wrapper.read(&VAR_INT)?.0;
    if connection.get::<StepStorage>().is_none() {
        connection.put(StepStorage::default());
    }
    connection
        .get_mut::<StepStorage>()
        .expect("teleport storage was just inserted")
        .pending_teleport_id = Some(id);
    wrapper.cancel();
    Ok(())
}

fn complete_teleportation(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let Some(id) = connection
        .get_mut::<StepStorage>()
        .and_then(|storage| storage.pending_teleport_id.take())
    else {
        wrapper.passthrough_all();
        return Ok(());
    };

    wrapper.set_packet(&serverbound::play::ACCEPT_TELEPORTATION);
    wrapper.write(&VAR_INT, &VarInt(id))?;
    for _ in 0..3 {
        wrapper.passthrough(&F64)?;
    }
    wrapper.passthrough(&F32)?; // Y rotation
    wrapper.passthrough(&F32)?; // X rotation
    wrapper.read(&U8)?; // on-ground flag is not part of 26.3's packet
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{I16, VAR_INT};
    use crate::api::{MappingData, Protocol, UserConnection};
    use crate::packet::mappings::{clientbound, serverbound};
    use pumpkin_protocol::ser::NetworkWriteExt;

    fn ctx(layout: JavaMinecraftVersion) -> Ctx<'static> {
        let step = super::super::Protocol26_3To26_2.step();
        // This step's static map is process-global and has a stable lifetime.
        let mappings = MappingData::get().step(step.from);
        Ctx {
            step,
            mappings,
            layout,
        }
    }

    #[test]
    fn movement_steps_are_summed_and_on_ground_moves_to_the_end() {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(9)).unwrap();
        VAR_INT.write(&mut payload, &VarInt((2 << 1) | 1)).unwrap();
        for (tick, values) in [(1, [2i16, -3, 4]), (2, [5, 6, -7])] {
            VAR_INT.write(&mut payload, &VarInt(tick)).unwrap();
            for value in values {
                I16.write(&mut payload, &value).unwrap();
            }
        }
        let mut wrapper = PacketWrapper::new(&clientbound::play::MOVE_ENTITY_POS, &payload);
        move_position(
            &mut wrapper,
            &mut UserConnection::new(1, JavaMinecraftVersion::V_26_2),
            &ctx(JavaMinecraftVersion::V_26_2),
        )
        .unwrap();
        let out = wrapper.finish().unwrap().unwrap();
        let mut cursor = out.payload.as_slice();
        assert_eq!(cursor.get_var_int().unwrap().0, 9);
        assert_eq!(cursor.get_i16_be().unwrap(), 7);
        assert_eq!(cursor.get_i16_be().unwrap(), 3);
        assert_eq!(cursor.get_i16_be().unwrap(), -3);
        assert_eq!(cursor.get_bool().unwrap(), true);
        assert!(cursor.is_empty());
    }

    #[test]
    fn no_swing_animation_is_cancelled_and_visible_swings_become_animate() {
        let mut none = PacketWrapper::new(&clientbound::play::SWING_ANIMATION, &[1, 0, 0, 6]);
        swing_animation(
            &mut none,
            &mut UserConnection::new(2, JavaMinecraftVersion::V_26_2),
            &ctx(JavaMinecraftVersion::V_26_2),
        )
        .unwrap();
        assert!(none.finish().unwrap().is_none());

        let mut packet = Vec::new();
        VAR_INT.write(&mut packet, &VarInt(3)).unwrap();
        VAR_INT.write(&mut packet, &VarInt(1)).unwrap();
        VAR_INT.write(&mut packet, &VarInt(2)).unwrap();
        VAR_INT.write(&mut packet, &VarInt(5)).unwrap();
        let mut wrapper = PacketWrapper::new(&clientbound::play::SWING_ANIMATION, &packet);
        swing_animation(
            &mut wrapper,
            &mut UserConnection::new(3, JavaMinecraftVersion::V_26_2),
            &ctx(JavaMinecraftVersion::V_26_2),
        )
        .unwrap();
        let out = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            std::ptr::from_ref(out.packet),
            std::ptr::from_ref(&clientbound::play::ANIMATE)
        );
        assert_eq!(out.payload, [3, 3]);
    }

    #[test]
    fn teleport_accept_is_coalesced_with_the_next_position_rotation() {
        let version = JavaMinecraftVersion::V_26_2;
        let mut connection = UserConnection::new(4, version);
        let mut accepted = PacketWrapper::new(&serverbound::play::ACCEPT_TELEPORTATION, &[42]);
        accept_teleportation(&mut accepted, &mut connection, &ctx(version)).unwrap();
        assert!(accepted.finish().unwrap().is_none());

        let mut position = Vec::new();
        for coordinate in [1.0f64, 2.0, 3.0] {
            position.write_f64_be(coordinate).unwrap();
        }
        position.write_f32_be(4.0).unwrap();
        position.write_f32_be(5.0).unwrap();
        position.push(1);
        let mut wrapper = PacketWrapper::new(&serverbound::play::MOVE_PLAYER_POS_ROT, &position);
        complete_teleportation(&mut wrapper, &mut connection, &ctx(version)).unwrap();
        let out = wrapper.finish().unwrap().unwrap();
        assert_eq!(
            std::ptr::from_ref(out.packet),
            std::ptr::from_ref(&serverbound::play::ACCEPT_TELEPORTATION)
        );
        let mut cursor = out.payload.as_slice();
        assert_eq!(cursor.get_var_int().unwrap().0, 42);
        for expected in [1.0, 2.0, 3.0] {
            assert_eq!(cursor.get_f64_be().unwrap(), expected);
        }
        assert_eq!(cursor.get_f32_be().unwrap(), 4.0);
        assert_eq!(cursor.get_f32_be().unwrap(), 5.0);
        assert!(cursor.is_empty());
    }
}
