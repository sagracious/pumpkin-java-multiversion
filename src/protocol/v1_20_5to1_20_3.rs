use crate::api::types::{ItemT, U8, VAR_INT};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection};
use crate::packet::mappings::clientbound;
use crate::registry;
use pumpkin_util::version::JavaMinecraftVersion;

/// `body`, the animal armour slot 1.20.5 added.
const BODY: u8 = 6;

pub struct Protocol1_20_5To1_20_3;

impl Protocol for Protocol1_20_5To1_20_3 {
    fn step(&self) -> Step {
        Step {
            from: JavaMinecraftVersion::V_1_20_5,
            to: JavaMinecraftVersion::V_1_20_3,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::SET_EQUIPMENT, equipment);
        reg.clientbound_layout(&clientbound::config::REGISTRY_DATA, collect);
        reg.clientbound(&clientbound::config::UPDATE_TAGS, flush);
        reg.clientbound(&clientbound::config::FINISH_CONFIGURATION, flush);
    }
}

/// The client indexes its slot enum with the raw value, so a slot it does not
/// have is not a wrong item but a crash. The list has no count and ends at the
/// first entry without the continuation bit, so the terminator moves with it.
fn equipment(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    if ctx.layout < JavaMinecraftVersion::V_1_16 {
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.passthrough(&VAR_INT)?;

    let item = ItemT::for_version(ctx.layout);
    let mut kept = Vec::new();
    loop {
        let slot = wrapper.read(&U8)?;
        let value = wrapper.read(&item)?;
        if slot & 0x7F != BODY {
            kept.push((slot & 0x7F, value));
        }
        if slot & 0x80 == 0 {
            break;
        }
    }
    if kept.is_empty() {
        wrapper.cancel();
        return Ok(());
    }

    let last = kept.len() - 1;
    for (index, (slot, value)) in kept.iter().enumerate() {
        let slot = if index == last { *slot } else { slot | 0x80 };
        wrapper.write(&U8, &slot)?;
        wrapper.write(&item, value)?;
    }
    Ok(())
}

/// The registry packets seen so far, waiting for the packet that follows them.
struct Pending(Vec<Vec<u8>>);

/// 1.20.4 and below take every registry in one packet, so each per registry
/// packet is held back until the configuration moves on.
fn collect(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let payload = wrapper.remaining().to_vec();
    match connection.get_mut::<Pending>() {
        Some(pending) => pending.0.push(payload),
        None => connection.put(Pending(vec![payload])),
    }
    wrapper.cancel();
    Ok(())
}

/// Sends the bundle in front of whatever ended the run of registry packets,
/// which keeps the order the client expects.
fn flush(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let Some(pending) = connection.get_mut::<Pending>() else {
        wrapper.passthrough_all();
        return Ok(());
    };
    let packets = std::mem::take(&mut pending.0);
    if packets.is_empty() {
        wrapper.passthrough_all();
        return Ok(());
    }

    let bundle = registry::bundle_registry_packets(connection.version, &packets)
        .ok_or(TranslateError::Unsupported("registry bundle"))?;
    let follower = wrapper.packet();
    let payload = wrapper.remaining().to_vec();
    wrapper.send_extra(follower, payload);
    wrapper.set_packet(&clientbound::config::REGISTRY_DATA);
    wrapper.replace_remaining(bundle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::WireType;
    use crate::pipeline::translate_clientbound;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::java::client::play::CSetEquipment;

    const PLAY: u8 = 5;

    fn written_by_core(version: JavaMinecraftVersion, slots: &[(i8, &'static Item)]) -> Vec<u8> {
        let equipment = slots
            .iter()
            .map(|(slot, item)| (*slot, ItemStackSerializer::from(ItemStack::new(1, item))))
            .collect();
        let packet = CSetEquipment::new(VarInt(42), equipment);
        let mut bytes = Vec::new();
        packet.write_packet_data(&mut bytes, &version).unwrap();
        bytes
    }

    fn translate(version: JavaMinecraftVersion, payload: &[u8]) -> Option<Vec<u8>> {
        translate_clientbound(
            0,
            version,
            PLAY,
            clientbound::play::SET_EQUIPMENT.v26_3,
            payload,
        )
        .map(|out| out.payload)
    }

    /// `md('1.20.2').protocol.play.toClient.packet_entity_equipment`: a
    /// `topBitSetTerminatedArray` of slot byte and item.
    #[test]
    fn the_body_slot_leaves_the_list_and_the_terminator_moves() {
        let version = JavaMinecraftVersion::V_1_20_3;
        let payload = written_by_core(
            version,
            &[
                (BODY as i8, &Item::DIAMOND_HORSE_ARMOR),
                (4, &Item::DIAMOND_CHESTPLATE),
            ],
        );
        let out = translate(version, &payload).unwrap();

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 42);
        assert_eq!(U8.read(&mut read).unwrap(), 4, "the only entry left");
        ItemT::for_version(version).read(&mut read).unwrap();
        assert!(read.is_empty());
    }

    #[test]
    fn a_body_only_packet_is_dropped() {
        let version = JavaMinecraftVersion::V_1_20_3;
        let payload = written_by_core(version, &[(BODY as i8, &Item::DIAMOND_HORSE_ARMOR)]);
        assert!(translate(version, &payload).is_none());
    }

    #[test]
    fn the_body_slot_is_kept_from_1_20_5() {
        let version = JavaMinecraftVersion::V_1_20_5;
        let payload = written_by_core(version, &[(BODY as i8, &Item::DIAMOND_HORSE_ARMOR)]);
        let out = translate(version, &payload).unwrap();

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 42);
        assert_eq!(U8.read(&mut read).unwrap(), BODY);
    }

    /// The whole tier below 1.20.5 carries the same list, item form aside.
    #[test]
    fn the_terminator_is_rebuilt_on_the_nbt_form_too() {
        let version = JavaMinecraftVersion::V_1_16_2;
        let payload = written_by_core(
            version,
            &[
                (2, &Item::DIAMOND_BOOTS),
                (BODY as i8, &Item::DIAMOND_HORSE_ARMOR),
                (4, &Item::DIAMOND_CHESTPLATE),
            ],
        );
        let out = translate(version, &payload).unwrap();

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 42);
        assert_eq!(U8.read(&mut read).unwrap(), 2 | 0x80);
        ItemT::for_version(version).read(&mut read).unwrap();
        assert_eq!(U8.read(&mut read).unwrap(), 4);
        ItemT::for_version(version).read(&mut read).unwrap();
        assert!(read.is_empty());
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::api::MappingData;
    use crate::registry::generated;
    use pumpkin_data::registry::RegistryEntryData;
    use pumpkin_protocol::ClientPacket;
    use pumpkin_protocol::java::client::config::CRegistryData;

    fn ctx() -> Ctx<'static> {
        let step = Protocol1_20_5To1_20_3.step();
        Ctx {
            step,
            mappings: MappingData::get().step(step.from),
            layout: JavaMinecraftVersion::V_1_20_5,
        }
    }

    /// One registry packet as core writes it, with the entries this version
    /// has so the bundle keeps them all.
    fn registry_packet(registry_id: &str, names: &[&str]) -> Vec<u8> {
        let entries: Vec<_> = names
            .iter()
            .map(|name| RegistryEntryData {
                entry_id: format!("minecraft:{name}"),
                data: Some(Box::new([0x0a, 0x00])),
            })
            .collect();
        let id = registry_id.to_string();
        let mut payload = Vec::new();
        CRegistryData::new(&id, &entries)
            .write_packet_data(&mut payload, &JavaMinecraftVersion::V_1_20_5)
            .unwrap();
        payload
    }

    /// minecraft-data types the configuration `registry_data` packet as a
    /// single `anonymousNbt` codec on `pc/1.20.2` and `pc/1.20.3`, against one
    /// packet per registry from `pc/1.20.5`.
    #[test]
    fn the_registries_arrive_as_one_bundle_in_front_of_the_tags() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let mut connection = UserConnection::new(0, version);
        let ctx = ctx();

        for payload in [
            registry_packet("minecraft:dimension_type", &["overworld"]),
            registry_packet("minecraft:worldgen/biome", &["plains", "pale_garden"]),
        ] {
            let mut wrapper = PacketWrapper::new(&clientbound::config::REGISTRY_DATA, &payload);
            collect(&mut wrapper, &mut connection, &ctx).unwrap();
            assert!(wrapper.finish().unwrap().is_none(), "held back");
        }

        let tags = b"\x00".to_vec();
        let mut wrapper = PacketWrapper::new(&clientbound::config::UPDATE_TAGS, &tags);
        flush(&mut wrapper, &mut connection, &ctx).unwrap();
        let out = wrapper.finish().unwrap().unwrap();

        assert_eq!(
            std::ptr::from_ref(out.packet),
            std::ptr::from_ref(&clientbound::config::REGISTRY_DATA)
        );
        assert_eq!(out.extra.len(), 1);
        assert_eq!(out.extra[0].1, tags, "the tags follow the bundle");

        let codec = crate::registry::read_unnamed_compound(&out.payload).expect("bundle parses");
        let biome = codec
            .get_compound("minecraft:worldgen/biome")
            .expect("biome registry");
        let values = biome.get_list("value").expect("value list");
        assert_eq!(values.len(), 2, "entry order and count kept");
        let plains = generated::get_synced(version)
            .unwrap()
            .iter()
            .find(|r| r.registry_id == "worldgen/biome")
            .unwrap()
            .entries
            .iter()
            .find(|e| e.name == "plains")
            .unwrap();
        for (index, name) in ["minecraft:plains", "minecraft:pale_garden"]
            .into_iter()
            .enumerate()
        {
            let value = values[index].extract_compound().unwrap();
            assert_eq!(value.get_string("name"), Some(name));
            assert_eq!(value.get_int("id"), Some(i32::try_from(index).unwrap()));
            assert_eq!(
                value.get("element"),
                Some(&crate::registry::read_tag(plains.data).unwrap()),
                "this version's own NBT, the unknown biome through the stand-in"
            );
        }
    }

    #[test]
    fn a_configuration_without_registries_is_left_alone() {
        let mut connection = UserConnection::new(0, JavaMinecraftVersion::V_1_20_2);
        let mut wrapper = PacketWrapper::new(&clientbound::config::FINISH_CONFIGURATION, &[]);
        flush(&mut wrapper, &mut connection, &ctx()).unwrap();
        let out = wrapper.finish().unwrap().unwrap();
        assert!(out.extra.is_empty());
        assert_eq!(
            std::ptr::from_ref(out.packet),
            std::ptr::from_ref(&clientbound::config::FINISH_CONFIGURATION)
        );
    }
}
