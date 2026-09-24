//! Rewrites the modern recipe-book display payload emitted by Pumpkin.
//!
//! Pumpkin's 26.3 recipe writer emits the stable empty, any-fuel, item,
//! item-stack and composite slot displays. Unknown display codecs fail closed:
//! their payload shape cannot safely be skipped without a matching Via handler.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::item::StructuredItemRewriter;
use crate::api::types::{BOOL, F32T, ItemT, STRING, TEMPLATE_ITEM, U8, VAR_INT};
use crate::api::{Ctx, MappingData, PacketWrapper, TranslateError, UserConnection};
use crate::data::mappings::{ComposedMappings, IdMapping};

const MAX_RECIPE_ENTRIES: i32 = 16_384;
const MAX_RECIPE_LIST: i32 = 4_096;

// These are the 26.2 slot-display ids used by Pumpkin's recipe packet writer.
const SLOT_DISPLAY_EMPTY: i32 = 0;
const SLOT_DISPLAY_ANY_FUEL: i32 = 1;
const SLOT_DISPLAY_ITEM: i32 = 4;
const SLOT_DISPLAY_ITEM_STACK: i32 = 5;
const SLOT_DISPLAY_COMPOSITE: i32 = 10;

/// Rewrites a 26.3 recipe-book payload directly to the connected client's
/// supported recipe-display version. The packet is not available before 1.21.2.
pub fn rewrite_recipe_book_add(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let target = connection.version;
    if target < V::V_1_21_2 {
        wrapper.cancel();
        return Ok(());
    }
    if ctx.layout != V::V_26_3 {
        return Err(TranslateError::Unsupported("recipe book source layout"));
    }

    let mappings = MappingData::get().composed(target);
    let entry_count = read_count(wrapper, MAX_RECIPE_ENTRIES, "recipe entry count", true)?;
    for _ in 0..entry_count {
        wrapper.passthrough(&VAR_INT)?; // Display id (referenced by Place Recipe)
        recipe_display(wrapper, target, mappings)?;
        wrapper.passthrough(&VAR_INT)?; // Optional group id
        wrapper.passthrough(&VAR_INT)?; // Recipe-book category
        crafting_requirements(wrapper, mappings)?;
        wrapper.passthrough(&U8)?; // Notification/highlight flags
    }
    Ok(())
}

fn read_count(
    wrapper: &mut PacketWrapper,
    maximum: i32,
    description: &'static str,
    emit: bool,
) -> Result<usize, TranslateError> {
    let count = wrapper.read(&VAR_INT)?.0;
    if !(0..=maximum).contains(&count) {
        return Err(TranslateError::Unsupported(description));
    }
    if emit {
        wrapper.write(&VAR_INT, &VarInt(count))?;
    }
    usize::try_from(count).map_err(|_| TranslateError::Unsupported(description))
}

fn mapped_id(mapping: &IdMapping, id: i32) -> Option<i32> {
    let id = u32::try_from(id).ok()?;
    i32::try_from(mapping.map(id)?).ok()
}

fn write_var_int(wrapper: &mut PacketWrapper, value: i32) -> Result<(), TranslateError> {
    wrapper.write(&VAR_INT, &VarInt(value))
}

fn recipe_display(
    wrapper: &mut PacketWrapper,
    target: V,
    mappings: &ComposedMappings,
) -> Result<(), TranslateError> {
    let kind = wrapper.passthrough(&VAR_INT)?.0;
    match kind {
        // Shapeless: ingredients, result and crafting station.
        0 => {
            slot_display_list(wrapper, target, mappings)?;
            slot_display(wrapper, target, mappings, true)?;
            slot_display(wrapper, target, mappings, true)?;
        }
        // Shaped: width, height, ingredients, result and crafting station.
        1 => {
            wrapper.passthrough(&VAR_INT)?;
            wrapper.passthrough(&VAR_INT)?;
            slot_display_list(wrapper, target, mappings)?;
            slot_display(wrapper, target, mappings, true)?;
            slot_display(wrapper, target, mappings, true)?;
        }
        // Furnace: ingredient, fuel, result, station, duration and experience.
        2 => {
            for _ in 0..4 {
                slot_display(wrapper, target, mappings, true)?;
            }
            wrapper.passthrough(&VAR_INT)?;
            wrapper.passthrough(&F32T)?;
        }
        // Stonecutter: input, result and station.
        3 => {
            for _ in 0..3 {
                slot_display(wrapper, target, mappings, true)?;
            }
        }
        // Smithing: template, base, addition, result and station.
        4 => {
            for _ in 0..5 {
                slot_display(wrapper, target, mappings, true)?;
            }
        }
        _ => return Err(TranslateError::Unsupported("recipe display type")),
    }
    Ok(())
}

fn slot_display_list(
    wrapper: &mut PacketWrapper,
    target: V,
    mappings: &ComposedMappings,
) -> Result<(), TranslateError> {
    let count = read_count(wrapper, MAX_RECIPE_LIST, "recipe slot display count", true)?;
    for _ in 0..count {
        slot_display(wrapper, target, mappings, true)?;
    }
    Ok(())
}

/// `emit=false` consumes a known display codec without writing it, as Via does
/// when the target version has no mapping for that display type.
fn slot_display(
    wrapper: &mut PacketWrapper,
    target: V,
    mappings: &ComposedMappings,
    emit: bool,
) -> Result<(), TranslateError> {
    let source_type = wrapper.read(&VAR_INT)?.0;
    let mapped_type = mapped_id(&mappings.slot_displays, source_type);

    match source_type {
        SLOT_DISPLAY_EMPTY | SLOT_DISPLAY_ANY_FUEL => {
            if emit {
                write_var_int(wrapper, mapped_type.filter(|id| *id != 0).unwrap_or(0))?;
            }
        }
        SLOT_DISPLAY_ITEM => {
            let source_item = wrapper.read(&VAR_INT)?.0;
            let mapped_item = mapped_id(&mappings.items, source_item);
            let display_type = if mapped_type.is_some_and(|id| id != 0) && mapped_item.is_some() {
                mapped_type.unwrap()
            } else {
                0
            };
            if emit {
                write_var_int(wrapper, display_type)?;
                if display_type != 0 {
                    write_var_int(wrapper, mapped_item.unwrap())?;
                }
            }
        }
        SLOT_DISPLAY_ITEM_STACK => {
            let native = wrapper.read(&TEMPLATE_ITEM)?;
            let mapped = StructuredItemRewriter::to_version(&native, target, mappings);
            let display_type = if mapped_type.is_some_and(|id| id != 0) && !mapped.is_empty() {
                mapped_type.unwrap()
            } else {
                0
            };
            if emit {
                write_var_int(wrapper, display_type)?;
                if display_type != 0 {
                    if target >= V::V_26_1 {
                        wrapper.write(&TEMPLATE_ITEM, &mapped)?;
                    } else {
                        wrapper.write(&ItemT::for_version(target), &mapped)?;
                    }
                }
            }
        }
        SLOT_DISPLAY_COMPOSITE => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            let emit_children = emit && display_type != 0;
            let count = read_count(
                wrapper,
                MAX_RECIPE_LIST,
                "composite slot display count",
                emit_children,
            )?;
            for _ in 0..count {
                slot_display(wrapper, target, mappings, emit_children)?;
            }
        }
        _ => return Err(TranslateError::Unsupported("recipe slot display codec")),
    }
    Ok(())
}

fn crafting_requirements(
    wrapper: &mut PacketWrapper,
    mappings: &ComposedMappings,
) -> Result<(), TranslateError> {
    if !wrapper.passthrough(&BOOL)? {
        return Ok(());
    }
    let count = read_count(
        wrapper,
        MAX_RECIPE_LIST,
        "recipe crafting requirement count",
        true,
    )?;
    for _ in 0..count {
        holder_set(wrapper, &mappings.items)?;
    }
    Ok(())
}

/// HolderSet uses selector 0 plus a tag string, or selector n+1 plus n ids.
/// An item id absent from the target registry is filtered out, never copied as
/// an accidental identity mapping.
fn holder_set(wrapper: &mut PacketWrapper, items: &IdMapping) -> Result<(), TranslateError> {
    let selector = wrapper.read(&VAR_INT)?.0;
    if selector == 0 {
        write_var_int(wrapper, 0)?;
        let tag = wrapper.read(&STRING)?;
        wrapper.write(&STRING, &tag)?;
        return Ok(());
    }
    if selector < 0 || selector - 1 > MAX_RECIPE_LIST {
        return Err(TranslateError::Unsupported("recipe item holder set"));
    }

    let mut mapped = Vec::new();
    for _ in 0..(selector - 1) {
        let id = wrapper.read(&VAR_INT)?.0;
        if let Some(id) = mapped_id(items, id) {
            mapped.push(id);
        }
    }
    let selector = i32::try_from(mapped.len() + 1)
        .map_err(|_| TranslateError::Unsupported("recipe item holder set"))?;
    write_var_int(wrapper, selector)?;
    for id in mapped {
        write_var_int(wrapper, id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::remove_connection;
    use crate::api::types::{Item, ItemComponent, WireType};
    use crate::packet::mappings::clientbound::play::RECIPE_BOOK_ADD;
    use crate::pipeline::translate_clientbound;
    use pumpkin_data::data_component::DataComponent;

    const PLAY: u8 = 5;
    const TARGET: V = V::V_26_2;

    fn mapped_source_item(mappings: &ComposedMappings) -> (i32, i32) {
        let source = i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id);
        let target = mapped_id(&mappings.items, source)
            .expect("diamond sword must exist in the target item registry");
        (source, target)
    }

    fn recipe_payload(item_id: i32) -> Vec<u8> {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // entries
        VAR_INT.write(&mut payload, &VarInt(17)).unwrap(); // display id
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // shapeless
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // ingredients
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_COMPOSITE))
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(2)).unwrap(); // composite children
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_ITEM))
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(item_id)).unwrap();
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_ITEM))
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(item_id)).unwrap();

        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_ITEM_STACK))
            .unwrap();
        TEMPLATE_ITEM
            .write(
                &mut payload,
                &Item::Structured {
                    count: 3,
                    id: item_id,
                    added: vec![ItemComponent {
                        id: i32::from(DataComponent::AttackAnimation.to_id()),
                        data: vec![1, 2],
                    }],
                    removed: Vec::new(),
                },
            )
            .unwrap();
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_ITEM))
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(item_id)).unwrap(); // station
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // no group
        VAR_INT.write(&mut payload, &VarInt(3)).unwrap(); // category
        BOOL.write(&mut payload, &true).unwrap();
        VAR_INT.write(&mut payload, &VarInt(2)).unwrap(); // requirement entries
        VAR_INT.write(&mut payload, &VarInt(2)).unwrap(); // one direct item id
        VAR_INT.write(&mut payload, &VarInt(item_id)).unwrap();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // named tag
        STRING
            .write(&mut payload, &"minecraft:stone_crafting_materials".into())
            .unwrap();
        U8.write(&mut payload, &3).unwrap(); // notification + highlight
        payload
    }

    #[test]
    fn recipe_displays_rewrite_nested_item_ids_components_and_holder_sets() {
        let key = 0x2623_0001;
        let mappings = MappingData::get().composed(TARGET);
        let (source_item, target_item) = mapped_source_item(mappings);
        let payload = recipe_payload(source_item);
        let translated = translate_clientbound(key, TARGET, PLAY, RECIPE_BOOK_ADD.v26_3, &payload)
            .expect("26.2 recipe book should translate");

        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(17));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mappings
                .slot_displays
                .map(SLOT_DISPLAY_COMPOSITE as u32)
                .unwrap() as i32
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        for _ in 0..2 {
            assert_eq!(
                VAR_INT.read(&mut cursor).unwrap().0,
                mappings
                    .slot_displays
                    .map(SLOT_DISPLAY_ITEM as u32)
                    .unwrap() as i32
            );
            assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(target_item));
        }
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mappings
                .slot_displays
                .map(SLOT_DISPLAY_ITEM_STACK as u32)
                .unwrap() as i32
        );
        let Item::Structured {
            id, added, count, ..
        } = TEMPLATE_ITEM.read(&mut cursor).unwrap()
        else {
            panic!("expected a translated structured result item");
        };
        assert_eq!(id, target_item);
        assert_eq!(count, 3);
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0].id,
            i32::from(DataComponent::AttackAnimation.to_id())
        );

        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mappings
                .slot_displays
                .map(SLOT_DISPLAY_ITEM as u32)
                .unwrap() as i32
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(target_item));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(3));
        assert!(BOOL.read(&mut cursor).unwrap());
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(target_item));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(
            STRING.read(&mut cursor).unwrap(),
            "minecraft:stone_crafting_materials"
        );
        assert_eq!(U8.read(&mut cursor).unwrap(), 3);
        assert!(cursor.is_empty());
        assert_eq!(
            translated.packet.to_id(TARGET),
            RECIPE_BOOK_ADD.to_id(TARGET)
        );
        remove_connection(key);
    }

    #[test]
    fn recipe_item_stacks_use_the_pre_26_1_wire_layout_for_1_21_11_clients() {
        let key = 0x2623_0005;
        let target = V::V_1_21_11;
        let mappings = MappingData::get().composed(target);
        let source_item = i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id);
        let target_item = mapped_id(&mappings.items, source_item).unwrap();
        let payload = recipe_payload(source_item);
        let translated = translate_clientbound(key, target, PLAY, RECIPE_BOOK_ADD.v26_3, &payload)
            .expect("1.21.11 recipe book should translate");

        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(17));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mappings
                .slot_displays
                .map(SLOT_DISPLAY_COMPOSITE as u32)
                .unwrap() as i32
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        for _ in 0..2 {
            assert_eq!(
                VAR_INT.read(&mut cursor).unwrap().0,
                mappings
                    .slot_displays
                    .map(SLOT_DISPLAY_ITEM as u32)
                    .unwrap() as i32
            );
            assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(target_item));
        }
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mappings
                .slot_displays
                .map(SLOT_DISPLAY_ITEM_STACK as u32)
                .unwrap() as i32
        );
        let Item::Structured {
            id, count, added, ..
        } = ItemT::for_version(target).read(&mut cursor).unwrap()
        else {
            panic!("expected a translated structured result item");
        };
        assert_eq!(id, target_item);
        assert_eq!(count, 3);
        assert!(
            added.is_empty(),
            "26.2-only attack animation is not sent to 1.21.11"
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mappings
                .slot_displays
                .map(SLOT_DISPLAY_ITEM as u32)
                .unwrap() as i32
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(target_item));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(3));
        assert!(BOOL.read(&mut cursor).unwrap());
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(target_item));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(
            STRING.read(&mut cursor).unwrap(),
            "minecraft:stone_crafting_materials"
        );
        assert_eq!(U8.read(&mut cursor).unwrap(), 3);
        assert!(cursor.is_empty());
        remove_connection(key);
    }

    #[test]
    fn recipe_display_packet_is_dropped_before_1_21_2() {
        let key = 0x2623_0002;
        assert!(
            translate_clientbound(key, V::V_1_20_5, PLAY, RECIPE_BOOK_ADD.v26_3, &[0],).is_none()
        );
        remove_connection(key);
    }

    #[test]
    fn items_missing_from_the_target_become_empty_displays_and_holder_sets() {
        let key = 0x2623_0004;
        let payload = recipe_payload(70_000);
        let translated = translate_clientbound(key, TARGET, PLAY, RECIPE_BOOK_ADD.v26_3, &payload)
            .expect("an unmappable recipe item should not invalidate the packet");
        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(17));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap(),
            VarInt(SLOT_DISPLAY_COMPOSITE)
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        for _ in 0..2 {
            assert_eq!(
                VAR_INT.read(&mut cursor).unwrap(),
                VarInt(SLOT_DISPLAY_EMPTY)
            );
        }
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap(),
            VarInt(SLOT_DISPLAY_EMPTY)
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap(),
            VarInt(SLOT_DISPLAY_EMPTY)
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(3));
        assert!(BOOL.read(&mut cursor).unwrap());
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1)); // empty direct set
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(
            STRING.read(&mut cursor).unwrap(),
            "minecraft:stone_crafting_materials"
        );
        assert_eq!(U8.read(&mut cursor).unwrap(), 3);
        assert!(cursor.is_empty());
        remove_connection(key);
    }

    #[test]
    fn unknown_slot_display_codec_drops_the_whole_recipe_packet() {
        let key = 0x2623_0003;
        let payload = [1, 0, 0, 1, 99];
        assert!(
            translate_clientbound(key, TARGET, PLAY, RECIPE_BOOK_ADD.v26_3, &payload,).is_none()
        );
        remove_connection(key);
    }
}
