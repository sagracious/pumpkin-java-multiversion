use crate::api::types::{F32, F64, I8, I16, STRING, U8, VAR_INT, WireType};
use crate::api::{
    Ctx, MappingData, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection,
};
use crate::packet::mappings::clientbound;
use pumpkin_data::item::Item;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CEntityVelocity;
use pumpkin_protocol::{ClientPacket, PositionFlag, ServerPacket};
use pumpkin_util::version::JavaMinecraftVersion;

/// The window and slot a container set slot addresses the cursor with.
const CURSOR_CONTAINER: i8 = -1;
const CURSOR_SLOT: i16 = -1;

pub struct Protocol1_21_2To1_21;

impl Protocol for Protocol1_21_2To1_21 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_21_2,
            to: JavaMinecraftVersion::V_1_21,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound_layout(&clientbound::play::COOLDOWN, cooldown);
        reg.clientbound_layout(&clientbound::play::PLAYER_POSITION, player_position);
        reg.clientbound(&clientbound::play::SET_CURSOR_ITEM, cursor_item);
    }
}

/// 1.21.2 added delta movement and widened the relative flags in the
/// teleport packet. Older clients keep position/rotation flags and receive
/// absolute delta as the legacy entity-velocity packet.
fn player_position(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let mut input = wrapper.remaining();
    let packet = pumpkin_protocol::java::client::play::CPlayerPosition::read(
        &mut input,
        &JavaMinecraftVersion::V_26_3,
    )?;
    if !input.is_empty() {
        return Err(TranslateError::Unsupported("player position trailing data"));
    }

    let relative_delta = packet.relatives.iter().any(|flag| {
        matches!(
            flag,
            PositionFlag::DeltaX
                | PositionFlag::DeltaY
                | PositionFlag::DeltaZ
                | PositionFlag::RotateDelta
        )
    });
    let mut output = Vec::with_capacity(43);
    F64.write(&mut output, &packet.position.x)?;
    F64.write(&mut output, &packet.position.y)?;
    F64.write(&mut output, &packet.position.z)?;
    F32.write(&mut output, &packet.yaw)?;
    F32.write(&mut output, &packet.pitch)?;
    U8.write(
        &mut output,
        &((PositionFlag::get_bitfield(&packet.relatives) as u8) & 0x1f),
    )?;
    VAR_INT.write(&mut output, &packet.teleport_id)?;
    wrapper.replace_remaining(output);

    let delta = packet.delta;
    if !relative_delta
        && (delta.x != 0.0 || delta.y != 0.0 || delta.z != 0.0)
        && let Some(entity_id) = connection.entity_tracker.client_entity_id
    {
        let velocity = CEntityVelocity::new(VarInt(entity_id), delta);
        let mut payload = Vec::new();
        velocity.write_packet_data(&mut payload, &ctx.step.to)?;
        wrapper.send_extra(&clientbound::play::SET_ENTITY_MOTION, payload);
    }
    Ok(())
}

/// Cooldowns are per item below 1.21.2, so the group names the item it was put
/// on. A group that is no item has nothing to apply to.
fn cooldown(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let group = wrapper.read(&STRING)?;
    let items = &MappingData::get().composed(connection.version).items;
    let id = Item::from_registry_key(&group)
        .and_then(|item| items.map(u32::from(item.id)))
        .and_then(|id| i32::try_from(id).ok());
    let Some(id) = id else {
        wrapper.cancel();
        return Ok(());
    };
    wrapper.write(&VAR_INT, &VarInt(id))?;
    wrapper.passthrough(&VAR_INT)?;
    Ok(())
}

/// 1.21 has no packet for the cursor and takes it as a slot of no container.
fn cursor_item(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.write(&I8, &CURSOR_CONTAINER)?;
    if ctx.layout >= JavaMinecraftVersion::V_1_17_1 {
        wrapper.write(&VAR_INT, &VarInt(0))?;
    }
    wrapper.write(&I16, &CURSOR_SLOT)?;
    wrapper.passthrough_all();
    wrapper.set_packet(&clientbound::play::CONTAINER_SET_SLOT);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::WireType;
    use crate::pipeline::translate_clientbound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::java::client::play::CItemCooldown;

    const PLAY: u8 = 5;

    fn written_by_core(version: JavaMinecraftVersion) -> Vec<u8> {
        let packet = CItemCooldown::new("minecraft:ender_pearl".to_string(), VarInt(20));
        let mut bytes = Vec::new();
        packet.write_packet_data(&mut bytes, &version).unwrap();
        bytes
    }

    #[test]
    fn a_group_becomes_the_item_id_below_1_21_2() {
        let version = JavaMinecraftVersion::V_1_21;
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::COOLDOWN.v26_3,
            &written_by_core(version),
        )
        .unwrap();

        let expected = MappingData::get()
            .composed(version)
            .items
            .map(u32::from(Item::ENDER_PEARL.id))
            .unwrap();
        let mut read: &[u8] = &out.payload;
        assert_eq!(
            VAR_INT.read(&mut read).unwrap().0,
            i32::try_from(expected).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 20);
        assert!(read.is_empty());
    }

    #[test]
    fn a_group_stays_a_string_from_1_21_2() {
        let version = JavaMinecraftVersion::V_1_21_2;
        let payload = written_by_core(version);
        let out = translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::COOLDOWN.v26_3,
            &payload,
        )
        .unwrap();
        assert_eq!(out.payload, payload);
    }

    #[test]
    fn a_group_that_is_no_item_is_dropped() {
        let version = JavaMinecraftVersion::V_1_21;
        let packet = CItemCooldown::new("pumpkin:not_an_item".to_string(), VarInt(5));
        let mut payload = Vec::new();
        packet.write_packet_data(&mut payload, &version).unwrap();
        assert!(
            translate_clientbound(
                0,
                version,
                PLAY,
                clientbound::play::COOLDOWN.v26_3,
                &payload
            )
            .is_none()
        );
    }
}

#[cfg(test)]
mod player_tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::pipeline::translate_clientbound;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
    use pumpkin_protocol::java::client::play::CSetCursorItem;
    const PLAY: u8 = 5;

    /// minecraft-data 1.21.1 `packet_set_slot`: a window id, a state id, the
    /// slot and the stack; 1.16.2 has the same without the state id.
    #[test]
    fn the_cursor_becomes_a_slot_of_no_container() {
        for (version, head) in [
            (JavaMinecraftVersion::V_1_21, vec![0xffu8, 0x00, 0xff, 0xff]),
            (JavaMinecraftVersion::V_1_16_2, vec![0xff, 0xff, 0xff]),
        ] {
            let stack = ItemStackSerializer::from(Option::<ItemStack>::None);
            let mut payload = Vec::new();
            CSetCursorItem::new(&stack)
                .write_packet_data(&mut payload, &version)
                .unwrap();

            let out = translate_clientbound(
                60,
                version,
                PLAY,
                clientbound::play::SET_CURSOR_ITEM.v26_3,
                &payload,
            )
            .unwrap();
            assert_eq!(
                out.packet.to_id(version),
                clientbound::play::CONTAINER_SET_SLOT.to_id(version),
                "{version}"
            );
            assert_eq!(&out.payload[..head.len()], &head[..], "{version}");
            remove_connection(60);
        }
    }
}

#[cfg(test)]
mod player_position_tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::pipeline::translate_clientbound;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::PositionFlag;
    use pumpkin_protocol::java::client::play::CPlayerPosition;
    use pumpkin_util::math::vector3::Vector3;

    const PLAY: u8 = 5;

    #[test]
    fn native_teleport_matches_legacy_core_layouts() {
        let native = CPlayerPosition::new(
            VarInt(42),
            Vector3::new(10.0, 65.0, -4.0),
            Vector3::new(0.0, 0.0, 0.0),
            90.0,
            -15.0,
            vec![PositionFlag::X, PositionFlag::YRot],
        );
        let mut source = Vec::new();
        native
            .write_packet_data(&mut source, &JavaMinecraftVersion::V_26_3)
            .unwrap();

        for (index, version) in [
            JavaMinecraftVersion::V_1_16_2,
            JavaMinecraftVersion::V_1_16_4,
            JavaMinecraftVersion::V_1_17,
            JavaMinecraftVersion::V_1_19_3,
            JavaMinecraftVersion::V_1_19_4,
            JavaMinecraftVersion::V_1_20_3,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_21_2,
            JavaMinecraftVersion::V_26_2,
            JavaMinecraftVersion::V_26_3,
        ]
        .into_iter()
        .enumerate()
        {
            let key = 0x504a_4d30 + index as u64;
            let translated = translate_clientbound(
                key,
                version,
                PLAY,
                clientbound::play::PLAYER_POSITION.v26_3,
                &source,
            )
            .unwrap_or_else(|| panic!("{version} teleport was dropped"));
            let mut expected = Vec::new();
            native.write_packet_data(&mut expected, &version).unwrap();
            assert_eq!(
                translated.packet.to_id(version),
                clientbound::play::PLAYER_POSITION.to_id(version),
                "{version} packet id"
            );
            assert_eq!(translated.payload, expected, "{version} payload");
            remove_connection(key);
        }
    }
}
