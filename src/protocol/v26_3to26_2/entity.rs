use pumpkin_data::entity::EntityType;
use pumpkin_protocol::ClientPacket;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CSpawnEntity;
use pumpkin_protocol::ser::NetworkReadExt;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::entity_data::{EntityDataEntry, EntityDataListT, MetaValue};
use crate::api::types::{F32, F64, U8, VAR_INT, WireType};
use crate::api::{Ctx, MappingData, PacketWrapper, Registry, TranslateError, UserConnection};
use crate::data::entity_data_types::meta_data_type_id_for_name;
use crate::packet::mappings::{clientbound, serverbound};

#[derive(Default)]
struct StepStorage {
    pending_teleport_id: Option<i32>,
}

pub(super) fn register(reg: &mut Registry) {
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
    use crate::api::{MappingData, Protocol, UserConnection};
    use crate::packet::mappings::serverbound;
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
