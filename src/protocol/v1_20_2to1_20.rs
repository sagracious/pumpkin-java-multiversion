use pumpkin_data::entity::EntityType;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::rewriter::block::RawNbtT;
use crate::api::rewriter::entity::{read_spawn, replace, spawned_type};
use crate::api::types::VAR_INT;
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::legacy::CSpawnPlayer;
use crate::packet::mappings::clientbound;

pub struct Protocol1_20_2To1_20;

impl Protocol for Protocol1_20_2To1_20 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_20_2,
            to: JavaMinecraftVersion::V_1_20,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::ADD_ENTITY, add_entity);
        reg.clientbound_layout(&clientbound::play::TAG_QUERY, tag_query);
    }
}

/// `handleAddEntity` has no case for the player type up to 1.20.1, so other
/// players stay invisible without `SPAWN_PLAYER`.
fn add_entity(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if spawned_type(wrapper, connection) != Some(EntityType::PLAYER.id) {
        wrapper.passthrough_all();
        return Ok(());
    }
    let spawn = read_spawn(wrapper, ctx.layout)?;
    let player = CSpawnPlayer::new(
        spawn.entity_id,
        spawn.entity_uuid,
        spawn.position,
        spawn.yaw,
        spawn.pitch,
    );
    replace(
        wrapper,
        &clientbound::play::SPAWN_PLAYER,
        &player,
        ctx.layout,
    )
}

/// 1.20.1 reads the answer as a named root; a missing tag is the single
/// `TAG_End` byte on both sides.
fn tag_query(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let tag = wrapper.read(&RawNbtT::for_version(ctx.step.from))?;
    let (&tag_id, body) = tag
        .split_first()
        .ok_or(TranslateError::Unsupported("empty tag query answer"))?;
    wrapper.write_bytes(&[tag_id]);
    if tag_id != 0 {
        wrapper.write_bytes(&[0, 0]);
        wrapper.write_bytes(body);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use pumpkin_nbt::Nbt;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::java::client::play::CTagQueryResponse;

    fn run(payload: &[u8]) -> Vec<u8> {
        let step = Protocol1_20_2To1_20.step();
        let ctx = Ctx {
            step,
            mappings: MappingData::get().step(step.from),
            layout: JavaMinecraftVersion::V_1_20_2,
        };
        let mut wrapper = PacketWrapper::new(&clientbound::play::TAG_QUERY, payload);
        let mut connection = UserConnection::new(0, JavaMinecraftVersion::V_1_20);
        tag_query(&mut wrapper, &mut connection, &ctx).unwrap();
        wrapper.finish().unwrap().unwrap().payload
    }

    /// minecraft-data types `nbt_query_response.nbt` as `optionalNbt` on
    /// `pc/1.20` and `anonOptionalNbt` on `pc/1.20.2`, which is the root name
    /// core stopped writing.
    #[test]
    fn the_root_gains_an_empty_name_for_1_20_1() {
        let mut compound = NbtCompound::new();
        compound.put_string("id", "minecraft:chest".to_string());
        let nbt_bytes = Nbt::new(String::new(), compound).write_unnamed();

        let mut payload = Vec::new();
        CTagQueryResponse::new(VarInt(7), &nbt_bytes)
            .write_packet_data(&mut payload, &JavaMinecraftVersion::V_1_20_2)
            .unwrap();

        let out = run(&payload);
        assert_eq!(out[0], 7);
        assert_eq!(out[1], 0x0a, "still a compound");
        assert_eq!(&out[2..4], [0, 0], "empty root name spliced in");
        assert_eq!(&out[4..], &nbt_bytes[1..]);
    }

    #[test]
    fn a_missing_tag_stays_one_byte() {
        let mut payload = Vec::new();
        CTagQueryResponse::new(VarInt(3), &[0])
            .write_packet_data(&mut payload, &JavaMinecraftVersion::V_1_20_2)
            .unwrap();
        assert_eq!(run(&payload), [3, 0]);
    }
}
