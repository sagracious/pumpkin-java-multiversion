use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::item::{ClientItemT, StructuredItemRewriter, item_pass};
use crate::api::rewriter::item_backup::{restore_full_item, rewrite_hashed_item};
use crate::api::types::{
    BOOL, F32T, HASHED_ITEM, I8, I16T, I32T, I64T, ITEM_COST, STRING, TEMPLATE_ITEM,
    TextComponentT, U8, VAR_INT,
};
use crate::api::{ComposedMappings, PacketWrapper, TranslateError, UserConnection};

/// A varint from 1.21.2, one byte below it.
fn container_id(wrapper: &mut PacketWrapper, layout: V) -> Result<(), TranslateError> {
    if layout >= V::V_1_21_2 {
        wrapper.passthrough(&VAR_INT)?;
    } else {
        wrapper.passthrough(&U8)?;
    }
    Ok(())
}

pub fn container_content(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    container_id(wrapper, layout)?;
    let count = if layout >= V::V_1_17_1 {
        wrapper.passthrough(&VAR_INT)?;
        wrapper.passthrough(&VAR_INT)?.0
    } else {
        i32::from(wrapper.passthrough(&I16T)?)
    };
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("container slot count"));
    }
    for _ in 0..count {
        item_pass(wrapper, connection, layout, ids)?;
    }
    if layout >= V::V_1_17_1 {
        item_pass(wrapper, connection, layout, ids)?;
    }
    Ok(())
}

pub fn container_slot(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    container_id(wrapper, layout)?;
    if layout >= V::V_1_17_1 {
        wrapper.passthrough(&VAR_INT)?;
    }
    wrapper.passthrough(&I16T)?;
    item_pass(wrapper, connection, layout, ids)
}

pub fn cursor_item(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    item_pass(wrapper, connection, layout, ids)
}

pub fn player_inventory(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    item_pass(wrapper, connection, layout, ids)
}

/// From 1.16 the entries run until one without the continuation bit.
pub fn equipment(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout <= V::V_1_7_6 {
        wrapper.passthrough(&I32T)?;
    } else {
        wrapper.passthrough(&VAR_INT)?;
    }
    if layout >= V::V_1_16 {
        loop {
            let slot = wrapper.passthrough(&U8)?;
            item_pass(wrapper, connection, layout, ids)?;
            if slot & 0x80 == 0 {
                break;
            }
        }
        return Ok(());
    }
    if layout >= V::V_1_9 {
        wrapper.passthrough(&VAR_INT)?;
    } else {
        wrapper.passthrough(&I16T)?;
    }
    item_pass(wrapper, connection, layout, ids)
}

/// A trade's two costs are the item cost form from 1.20.5 and plain stacks
/// below it.
pub fn merchant_offers(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&VAR_INT)?;
    let offers = if layout >= V::V_1_19 {
        wrapper.passthrough(&VAR_INT)?.0
    } else {
        i32::from(wrapper.passthrough(&U8)?)
    };
    if !(0..=256).contains(&offers) {
        return Err(TranslateError::Unsupported("merchant offer count"));
    }
    for _ in 0..offers {
        if layout >= V::V_1_20_5 {
            cost(wrapper, connection, layout, ids)?;
            item_pass(wrapper, connection, layout, ids)?;
            if wrapper.passthrough(&BOOL)? {
                cost(wrapper, connection, layout, ids)?;
            }
        } else {
            // The second cost is a bare slot; an absent one is an empty stack,
            // which is the same single byte core writes for it.
            for _ in 0..3 {
                item_pass(wrapper, connection, layout, ids)?;
            }
        }
        wrapper.passthrough(&BOOL)?;
        for _ in 0..4 {
            wrapper.passthrough(&I32T)?;
        }
        wrapper.passthrough(&F32T)?;
        wrapper.passthrough(&I32T)?;
    }
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&VAR_INT)?;
    wrapper.passthrough(&BOOL)?;
    wrapper.passthrough(&BOOL)?;
    Ok(())
}

fn cost(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let item = wrapper.read(&ITEM_COST)?;
    let mut out = StructuredItemRewriter::to_version(&item, layout, ids);
    crate::api::rewriter::item_backup::backup_clientbound_item(
        connection, &item, &mut out, layout, ids,
    );
    wrapper.write(&ITEM_COST, &out)
}

pub fn advancements(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let text = TextComponentT::for_version(layout);
    wrapper.passthrough(&BOOL)?;
    let added = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=4096).contains(&added) {
        return Err(TranslateError::Unsupported("advancement count"));
    }
    for _ in 0..added {
        wrapper.passthrough(&STRING)?;
        if wrapper.passthrough(&BOOL)? {
            wrapper.passthrough(&STRING)?;
        }
        if wrapper.passthrough(&BOOL)? {
            wrapper.passthrough(&text)?;
            wrapper.passthrough(&text)?;
            icon(wrapper, connection, layout, ids)?;
            wrapper.passthrough(&VAR_INT)?;
            let flags = wrapper.passthrough(&I32T)?;
            if flags & 1 != 0 {
                wrapper.passthrough(&STRING)?;
            }
            if layout < V::V_26_3 {
                wrapper.passthrough(&F32T)?;
                wrapper.passthrough(&F32T)?;
            }
        }
        if layout < V::V_1_20_2 {
            strings(wrapper)?;
        }
        let requirements = wrapper.passthrough(&VAR_INT)?.0;
        for _ in 0..requirements {
            strings(wrapper)?;
        }
        if layout >= V::V_1_20 {
            wrapper.passthrough(&BOOL)?;
        }
        if layout >= V::V_26_3 {
            wrapper.passthrough(&F32T)?;
            wrapper.passthrough(&F32T)?;
        }
    }
    strings(wrapper)?;
    let progress = wrapper.passthrough(&VAR_INT)?.0;
    for _ in 0..progress {
        wrapper.passthrough(&STRING)?;
        let criteria = wrapper.passthrough(&VAR_INT)?.0;
        for _ in 0..criteria {
            wrapper.passthrough(&STRING)?;
            if wrapper.passthrough(&BOOL)? {
                wrapper.passthrough(&I64T)?;
            }
        }
    }
    if layout >= V::V_1_21_5 {
        wrapper.passthrough(&BOOL)?;
    }
    Ok(())
}

fn strings(wrapper: &mut PacketWrapper) -> Result<(), TranslateError> {
    let count = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("string list"));
    }
    for _ in 0..count {
        wrapper.passthrough(&STRING)?;
    }
    Ok(())
}

/// An advancement icon is the template form from 26.1 and a plain stack below.
fn icon(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout < V::V_26_1 {
        return item_pass(wrapper, connection, layout, ids);
    }
    let item = wrapper.read(&TEMPLATE_ITEM)?;
    let mut out = StructuredItemRewriter::to_version(&item, layout, ids);
    crate::api::rewriter::item_backup::backup_clientbound_item(
        connection, &item, &mut out, layout, ids,
    );
    wrapper.write(&TEMPLATE_ITEM, &out)
}

pub fn creative_slot(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I16T)?;
    let mut item = wrapper.read(&ClientItemT::new(layout, ids))?;
    restore_full_item(connection, &mut item, layout, ids);
    wrapper.write(&ClientItemT::new(layout, ids), &item)?;
    Ok(())
}

/// The fields ahead of the changed slot list. Core reads the container id and
/// the state id in the widths it branches on, so both follow the client.
pub fn click_frame(wrapper: &mut PacketWrapper, version: V) -> Result<(), TranslateError> {
    container_id(wrapper, version)?;
    if version >= V::V_1_17_1 {
        wrapper.passthrough(&VAR_INT)?;
    } else {
        wrapper.passthrough(&I16T)?;
    }
    wrapper.passthrough(&I16T)?;
    wrapper.passthrough(&I8)?;
    wrapper.passthrough(&VAR_INT)?;
    Ok(())
}

/// Every stack is a hash by the time the id pass runs: clients below 1.21.5
/// send whole ones and the step at that boundary has already hashed them.
pub fn click_container(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _layout: V,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    click_frame(wrapper, connection.version)?;
    let changed = wrapper.passthrough(&VAR_INT)?.0;
    if !(0..=256).contains(&changed) {
        return Err(TranslateError::Unsupported("changed slot count"));
    }
    let source = connection.version;
    for _ in 0..changed {
        wrapper.passthrough(&I16T)?;
        clicked(wrapper, connection, ids, source)?;
    }
    clicked(wrapper, connection, ids, source)
}

fn clicked(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ids: &ComposedMappings,
    source: V,
) -> Result<(), TranslateError> {
    let hashed = wrapper.read(&HASHED_ITEM)?;
    let out = hashed.and_then(|mut item| {
        rewrite_hashed_item(connection, &mut item, source, ids)?;
        Some(item)
    });
    wrapper.write(&HASHED_ITEM, &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MappingData;
    use crate::api::types::{Item, ItemComponent, ItemT, WireType};
    use crate::packet::mappings::{clientbound, serverbound};
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_protocol::codec::var_int::VarInt;

    fn ids(target: V) -> &'static ComposedMappings {
        MappingData::get().composed(target)
    }

    /// A sharpness 5 diamond sword in the 26.3 structured form core writes.
    fn native_sword() -> Item {
        Item::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
            added: vec![ItemComponent {
                id: i32::from(DataComponent::Enchantments.to_id()),
                data: vec![1, 12, 5],
            }],
            removed: Vec::new(),
        }
    }

    fn client_bytes(item: &Item, version: V) -> Vec<u8> {
        let rewritten = StructuredItemRewriter::to_version(item, version, ids(version));
        let mut out = Vec::new();
        ItemT::for_version(version)
            .write(&mut out, &rewritten)
            .unwrap();
        out
    }

    /// The two container frames core still writes itself, checked against its
    /// own writer on both sides of the 1.17.1 and 1.21.2 boundaries.
    #[test]
    fn the_container_frames_core_writes_are_the_ones_the_id_pass_reads() {
        use pumpkin_protocol::ClientPacket;
        use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
        use pumpkin_protocol::java::client::play::{CSetContainerContent, CSetContainerSlot};

        for version in [
            V::V_1_16_2,
            V::V_1_17,
            V::V_1_17_1,
            V::V_1_20_2,
            V::V_1_21_2,
            V::V_26_2,
        ] {
            let slots = [ItemStackSerializer::from(
                pumpkin_data::item_stack::ItemStack::new(1, &pumpkin_data::item::Item::STONE),
            )];
            let carried = ItemStackSerializer::from(pumpkin_data::item_stack::ItemStack::new(
                2,
                &pumpkin_data::item::Item::DIRT,
            ));
            let mut payload = Vec::new();
            CSetContainerContent::new(VarInt(3), VarInt(9), &slots, &carried)
                .write_packet_data(&mut payload, &version)
                .unwrap();
            let mut wrapper =
                PacketWrapper::new(&clientbound::play::CONTAINER_SET_CONTENT, &payload);
            let mut connection = UserConnection::new(0, version);
            container_content(&mut wrapper, &mut connection, version, ids(version)).unwrap();
            wrapper.finish().unwrap().unwrap();

            let mut payload = Vec::new();
            CSetContainerSlot::new(-1, 9, -1, &carried)
                .write_packet_data(&mut payload, &version)
                .unwrap();
            let mut wrapper = PacketWrapper::new(&clientbound::play::CONTAINER_SET_SLOT, &payload);
            let mut connection = UserConnection::new(0, version);
            container_slot(&mut wrapper, &mut connection, version, ids(version)).unwrap();
            wrapper.finish().unwrap().unwrap();
        }
    }

    /// `md('1.21.3').protocol.play.toClient.packet_window_items`: a varint
    /// window id, a state id, a varint slot count and the carried item.
    #[test]
    fn container_content_keeps_its_frame_and_rewrites_every_slot() {
        let layout = V::V_1_21_2;
        let mut payload = vec![3, 9, 2];
        payload.extend(client_bytes(&native_sword(), layout));
        payload.extend(client_bytes(&Item::Empty, layout));
        payload.extend(client_bytes(&Item::Empty, layout));

        let mut wrapper = PacketWrapper::new(&clientbound::play::CONTAINER_SET_CONTENT, &payload);
        let mut connection = UserConnection::new(0, layout);
        container_content(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 3);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 9);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 2);
        let sword = ItemT::for_version(layout).read(&mut read).unwrap();
        assert_eq!(
            sword.item_id(),
            Some(
                i32::try_from(
                    ids(layout)
                        .items
                        .map(u32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id))
                        .unwrap()
                )
                .unwrap()
            )
        );
    }

    #[test]
    fn container_content_translates_interact_animation_for_26_2() {
        let layout = V::V_26_2;
        let interactable = Item::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id),
            added: vec![ItemComponent {
                id: i32::from(DataComponent::InteractAnimation.to_id()),
                data: vec![1, 6],
            }],
            removed: Vec::new(),
        };
        let mut payload = vec![3, 9, 1];
        payload.extend(client_bytes(&interactable, layout));
        payload.extend(client_bytes(&Item::Empty, layout));

        let mut wrapper = PacketWrapper::new(&clientbound::play::CONTAINER_SET_CONTENT, &payload);
        let mut connection = UserConnection::new(0, layout);
        container_content(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 3);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 9);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        let item = ItemT::for_version(layout).read(&mut read).unwrap();
        let Item::Structured { added, .. } = item else {
            panic!("structured");
        };
        assert_eq!(added.len(), 2, "animation plus its recovery marker");
        let animation = added
            .iter()
            .find(|component| component.id == i32::from(DataComponent::AttackAnimation.to_id()))
            .unwrap();
        assert_eq!(animation.data, vec![1, 6]);
        assert!(
            added
                .iter()
                .any(|component| { component.id == i32::from(DataComponent::CustomData.to_id()) })
        );
        assert!(
            ItemT::for_version(layout)
                .read(&mut read)
                .unwrap()
                .is_empty()
        );
        assert!(read.is_empty());
    }

    /// `md('1.16.5').protocol.play.toClient.packet_window_items`: one byte of
    /// window id, a Short slot count and no carried item.
    #[test]
    fn container_content_on_1_16_has_a_short_count_and_no_carried_item() {
        let layout = V::V_1_16_2;
        let mut payload = vec![3, 0, 1];
        payload.extend(client_bytes(&native_sword(), layout));

        let mut wrapper = PacketWrapper::new(&clientbound::play::CONTAINER_SET_CONTENT, &payload);
        let mut connection = UserConnection::new(0, layout);
        container_content(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        assert_eq!(U8.read(&mut read).unwrap(), 3);
        assert_eq!(I16T.read(&mut read).unwrap(), 1);
        let sword = ItemT::for_version(layout).read(&mut read).unwrap();
        assert!(matches!(sword, Item::Nbt { .. }), "the nbt form");
        assert!(read.is_empty());
    }

    #[test]
    fn a_container_slot_keeps_the_state_id() {
        let layout = V::V_1_20_2;
        let mut payload = vec![1, 9, 0, 5];
        payload.extend(client_bytes(&native_sword(), layout));

        let mut wrapper = PacketWrapper::new(&clientbound::play::CONTAINER_SET_SLOT, &payload);
        let mut connection = UserConnection::new(0, layout);
        container_slot(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;
        assert_eq!(&out[..4], &[1, 9, 0, 5]);
    }

    /// `md('1.21.3').protocol.play.toClient.packet_entity_equipment`: the
    /// entries end at the first slot byte without 0x80.
    #[test]
    fn equipment_stops_at_the_terminator() {
        let layout = V::V_1_20_2;
        let mut payload = vec![42, 4 | 0x80];
        payload.extend(client_bytes(&native_sword(), layout));
        payload.push(3);
        payload.extend(client_bytes(&Item::Empty, layout));

        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_EQUIPMENT, &payload);
        let mut connection = UserConnection::new(0, layout);
        equipment(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 42);
        assert_eq!(U8.read(&mut read).unwrap(), 4 | 0x80);
        ItemT::for_version(layout).read(&mut read).unwrap();
        assert_eq!(U8.read(&mut read).unwrap(), 3);
        ItemT::for_version(layout).read(&mut read).unwrap();
        assert!(read.is_empty());
    }

    /// `md('1.21.3').protocol.play.toClient.packet_trade_list`: the first cost
    /// is item id, count and a component list, the second an optional one.
    #[test]
    fn merchant_offers_rewrite_both_cost_forms() {
        let layout = V::V_1_21_2;
        let sword = native_sword();
        let mut payload = vec![1, 1];
        ITEM_COST.write(&mut payload, &sword).unwrap();
        payload.extend(client_bytes(&sword, layout));
        payload.push(0);
        payload.push(1);
        payload.extend([0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4]);
        payload.extend([0, 0, 0, 0]);
        payload.extend([0, 0, 0, 5]);
        payload.extend([1, 0, 1, 0]);

        let mut wrapper = PacketWrapper::new(&clientbound::play::MERCHANT_OFFERS, &payload);
        let mut connection = UserConnection::new(0, layout);
        merchant_offers(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 1);
        let cost = ITEM_COST.read(&mut read).unwrap();
        assert!(cost.item_id().is_some());
    }

    /// The creative slot carries the whole stack; core now reads the 26.3
    /// form for every version.
    #[test]
    fn a_creative_slot_comes_back_in_the_26_3_form() {
        let layout = V::V_1_21_4;
        let client = StructuredItemRewriter::to_version(&native_sword(), layout, ids(layout));
        let mut payload = vec![0, 36];
        ItemT::for_version(layout)
            .write(&mut payload, &client)
            .unwrap();

        let mut wrapper = PacketWrapper::new(&serverbound::play::SET_CREATIVE_MODE_SLOT, &payload);
        let mut connection = UserConnection::new(0, layout);
        creative_slot(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        assert_eq!(I16T.read(&mut read).unwrap(), 36);
        let native = ItemT::for_version(V::V_26_3).read(&mut read).unwrap();
        assert_eq!(
            native.item_id(),
            Some(i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id))
        );
        assert!(read.is_empty());
    }

    /// `md('1.21.5').types.HashedSlot`: item id, count, component hashes and
    /// the removed component ids.
    #[test]
    fn a_click_maps_the_hashes_back_to_26_3_ids() {
        let layout = V::V_1_21_5;
        let sword = ids(layout)
            .items
            .map(u32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id))
            .unwrap();
        let mut payload = vec![1, 9, 0, 36, 0, 0, 0];
        payload.push(1);
        VAR_INT
            .write(&mut payload, &VarInt(i32::try_from(sword).unwrap()))
            .unwrap();
        payload.extend([1, 0, 0]);

        let mut wrapper = PacketWrapper::new(&serverbound::play::CONTAINER_CLICK, &payload);
        let mut connection = UserConnection::new(0, layout);
        click_container(&mut wrapper, &mut connection, layout, ids(layout)).unwrap();
        let out = wrapper.finish().unwrap().unwrap().payload;

        let mut read: &[u8] = &out;
        for _ in 0..2 {
            VAR_INT.read(&mut read).unwrap();
        }
        I16T.read(&mut read).unwrap();
        I8.read(&mut read).unwrap();
        VAR_INT.read(&mut read).unwrap();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0, "no changed slots");
        let carried = HASHED_ITEM.read(&mut read).unwrap().unwrap();
        assert_eq!(
            carried.id,
            i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id)
        );
        assert!(read.is_empty());
    }
}
