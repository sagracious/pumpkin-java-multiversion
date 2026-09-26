//! Rewrites Pumpkin's 26.3 recipe-book payload for an older client.
//!
//! Slot-display types and nested data are mapped to the target registry and
//! payload layout. Unknown display codecs fail closed: their payload shape
//! cannot safely be skipped without a matching Via handler.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::item::StructuredItemRewriter;
use crate::api::types::{BOOL, F32T, ItemT, NbtT, STRING, TEMPLATE_ITEM, U8, VAR_INT};
use crate::api::{Ctx, MappingData, PacketWrapper, TranslateError, UserConnection};
use crate::data::mappings::{ComposedMappings, IdMapping};

#[path = "recipe_legacy.rs"]
mod legacy;

const MAX_RECIPE_ENTRIES: i32 = 16_384;
const MAX_RECIPE_LIST: i32 = 4_096;

// Source 26.3 slot-display IDs (the first 11 IDs match 26.2).
const SLOT_DISPLAY_EMPTY: i32 = 0;
const SLOT_DISPLAY_ANY_FUEL: i32 = 1;
const SLOT_DISPLAY_WITH_ANY_POTION: i32 = 2;
const SLOT_DISPLAY_ONLY_WITH_COMPONENT: i32 = 3;
const SLOT_DISPLAY_ITEM: i32 = 4;
const SLOT_DISPLAY_ITEM_STACK: i32 = 5;
const SLOT_DISPLAY_TAG: i32 = 6;
const SLOT_DISPLAY_DYED: i32 = 7;
const SLOT_DISPLAY_SMITHING_TRIM: i32 = 8;
const SLOT_DISPLAY_WITH_REMAINDER: i32 = 9;
const SLOT_DISPLAY_COMPOSITE: i32 = 10;

/// Rewrites a 26.3 recipe-book payload directly to the connected client's
/// supported recipe-display version. The packet is not available before 1.21.2.
pub fn rewrite_recipe_book_add(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let target = connection.version;
    if ctx.layout != V::V_26_3 {
        return Err(TranslateError::Unsupported("recipe book source layout"));
    }
    if target < V::V_1_21_2 {
        return legacy::rewrite_recipe_book_add(wrapper, connection, target);
    }

    let mappings = MappingData::get().composed(target);
    let entry_count = read_count(wrapper, MAX_RECIPE_ENTRIES, "recipe entry count", true)?;
    for _ in 0..entry_count {
        wrapper.passthrough(&VAR_INT)?; // Display id (referenced by Place Recipe)
        recipe_display(wrapper, connection, target, mappings)?;
        wrapper.passthrough(&VAR_INT)?; // Optional group id
        wrapper.passthrough(&VAR_INT)?; // Recipe-book category
        crafting_requirements(wrapper, mappings)?;
        wrapper.passthrough(&U8)?; // Notification/highlight flags
    }
    wrapper.passthrough(&BOOL)?; // Replace the recipe-book contents
    Ok(())
}

/// Rewrites recipe groups and stonecutter displays. For pre-1.21.2 clients,
/// the legacy bridge retains stonecutter inputs until the next recipe-book add.
pub fn rewrite_update_recipes(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let target = connection.version;
    if ctx.layout != V::V_26_3 {
        return Err(TranslateError::Unsupported("update recipes source layout"));
    }
    if target < V::V_1_21_2 {
        return legacy::rewrite_update_recipes(wrapper, connection, target);
    }

    let mappings = MappingData::get().composed(target);
    let groups = read_count(wrapper, MAX_RECIPE_ENTRIES, "recipe group count", true)?;
    for _ in 0..groups {
        wrapper.passthrough(&STRING)?; // Recipe group
        let item_count = read_count(wrapper, MAX_RECIPE_LIST, "recipe group item count", true)?;
        for _ in 0..item_count {
            let item = wrapper.read(&VAR_INT)?.0;
            write_var_int(wrapper, mapped_id_or_identity(&mappings.items, item))?;
        }
    }

    let stonecutter_recipes = read_count(
        wrapper,
        MAX_RECIPE_ENTRIES,
        "stonecutter recipe count",
        true,
    )?;
    for _ in 0..stonecutter_recipes {
        holder_set(wrapper, &mappings.items)?;
        slot_display(wrapper, connection, target, mappings, true)?;
    }
    Ok(())
}

pub(crate) fn rewrite_legacy_recipe_book_remove(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
) -> Result<(), TranslateError> {
    legacy::rewrite_recipe_book_remove(wrapper, connection, layout)
}

pub(crate) fn rewrite_legacy_recipe_book_settings(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
) -> Result<(), TranslateError> {
    legacy::rewrite_recipe_book_settings(wrapper, connection, layout)
}

pub(crate) fn rewrite_legacy_place_recipe(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
) -> Result<(), TranslateError> {
    legacy::rewrite_place_recipe(wrapper, connection)
}

pub(crate) fn rewrite_legacy_seen_recipe(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
) -> Result<(), TranslateError> {
    legacy::rewrite_seen_recipe(wrapper, connection)
}

/// Rewrites the recipe display shown when an older client opens a recipe.
pub fn rewrite_place_ghost_recipe(
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
        return Err(TranslateError::Unsupported("ghost recipe source layout"));
    }

    wrapper.passthrough(&VAR_INT)?; // Container ID
    recipe_display(
        wrapper,
        connection,
        target,
        MappingData::get().composed(target),
    )
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

/// Via's item registry rewriter preserves IDs with no mapping row.
fn mapped_id_or_identity(mapping: &IdMapping, id: i32) -> i32 {
    mapped_id(mapping, id).unwrap_or(id)
}

fn write_var_int(wrapper: &mut PacketWrapper, value: i32) -> Result<(), TranslateError> {
    wrapper.write(&VAR_INT, &VarInt(value))
}

fn recipe_display(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Result<(), TranslateError> {
    let kind = wrapper.passthrough(&VAR_INT)?.0;
    match kind {
        // Shapeless: ingredients, result and crafting station.
        0 => {
            slot_display_list(wrapper, connection, target, mappings)?;
            slot_display(wrapper, connection, target, mappings, true)?;
            slot_display(wrapper, connection, target, mappings, true)?;
        }
        // Shaped: width, height, ingredients, result and crafting station.
        1 => {
            wrapper.passthrough(&VAR_INT)?;
            wrapper.passthrough(&VAR_INT)?;
            slot_display_list(wrapper, connection, target, mappings)?;
            slot_display(wrapper, connection, target, mappings, true)?;
            slot_display(wrapper, connection, target, mappings, true)?;
        }
        // Furnace: ingredient, fuel, result, station, duration and experience.
        2 => {
            for _ in 0..4 {
                slot_display(wrapper, connection, target, mappings, true)?;
            }
            wrapper.passthrough(&VAR_INT)?;
            wrapper.passthrough(&F32T)?;
        }
        // Stonecutter: input, result and station.
        3 => {
            for _ in 0..3 {
                slot_display(wrapper, connection, target, mappings, true)?;
            }
        }
        // Smithing: template, base, addition, result and station.
        4 => {
            for _ in 0..5 {
                slot_display(wrapper, connection, target, mappings, true)?;
            }
        }
        _ => return Err(TranslateError::Unsupported("recipe display type")),
    }
    Ok(())
}

fn slot_display_list(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Result<(), TranslateError> {
    let count = read_count(wrapper, MAX_RECIPE_LIST, "recipe slot display count", true)?;
    for _ in 0..count {
        slot_display(wrapper, connection, target, mappings, true)?;
    }
    Ok(())
}

/// `emit=false` consumes a known display codec without writing it, as Via does
/// when the target version has no mapping for that display type.
fn slot_display(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
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
            let mapped_item = mapped_id_or_identity(&mappings.items, source_item);
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
                if display_type != 0 {
                    write_var_int(wrapper, mapped_item)?;
                }
            }
        }
        SLOT_DISPLAY_ITEM_STACK => {
            let native = wrapper.read(&TEMPLATE_ITEM)?;
            let mut mapped = StructuredItemRewriter::to_version(&native, target, mappings);
            let display_type = if mapped_type.is_some_and(|id| id != 0) && !mapped.is_empty() {
                mapped_type.unwrap()
            } else {
                0
            };
            if emit {
                write_var_int(wrapper, display_type)?;
                if display_type != 0 {
                    crate::api::rewriter::item_backup::backup_clientbound_item(
                        connection,
                        &native,
                        &mut mapped,
                        target,
                        mappings,
                    );
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
                slot_display(wrapper, connection, target, mappings, emit_children)?;
            }
        }
        SLOT_DISPLAY_WITH_ANY_POTION => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            slot_display(
                wrapper,
                connection,
                target,
                mappings,
                emit && display_type != 0,
            )?;
        }
        SLOT_DISPLAY_ONLY_WITH_COMPONENT => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            let emit_children = emit && display_type != 0;
            slot_display(wrapper, connection, target, mappings, emit_children)?;
            let source_component = wrapper.read(&VAR_INT)?.0;
            if emit_children {
                let component =
                    mapped_id(&mappings.data_component_type, source_component).unwrap_or(0);
                write_var_int(wrapper, component)?;
            }
        }
        SLOT_DISPLAY_TAG => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            let emit_tag = emit && display_type != 0;
            // In the 26.3 layout this is a HolderSet. ViaBackwards converts
            // named sets to the old string form and uses "planks" when an
            // explicit ID set cannot be represented (Protocol26_3To26_2).
            let selector = wrapper.read(&VAR_INT)?.0;
            let tag = if selector == 0 {
                Some(wrapper.read(&STRING)?)
            } else {
                if selector < 0 || selector - 1 > MAX_RECIPE_LIST {
                    return Err(TranslateError::Unsupported("recipe slot display tag"));
                }
                for _ in 0..(selector - 1) {
                    wrapper.read(&VAR_INT)?;
                }
                Some("planks".into())
            };
            if emit_tag {
                wrapper.write(&STRING, &tag.expect("tag display has a payload"))?;
            }
        }
        SLOT_DISPLAY_DYED => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            let emit_children = emit && display_type != 0;
            for _ in 0..2 {
                slot_display(wrapper, connection, target, mappings, emit_children)?;
            }
        }
        SLOT_DISPLAY_SMITHING_TRIM => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            let emit_children = emit && display_type != 0;
            for _ in 0..2 {
                slot_display(wrapper, connection, target, mappings, emit_children)?;
            }
            // ArmorTrimPattern.TYPE1_21_5 is asset name, text NBT, and decal.
            // ViaBackwards passes this payload through unchanged.
            let asset_name = wrapper.read(&STRING)?;
            let description = wrapper.read(&NbtT::for_version(V::V_26_3))?;
            let decal = wrapper.read(&BOOL)?;
            if emit_children {
                wrapper.write(&STRING, &asset_name)?;
                wrapper.write(&NbtT::for_version(V::V_26_3), &description)?;
                wrapper.write(&BOOL, &decal)?;
            }
        }
        SLOT_DISPLAY_WITH_REMAINDER => {
            let display_type = mapped_type.filter(|id| *id != 0).unwrap_or(0);
            if emit {
                write_var_int(wrapper, display_type)?;
            }
            let emit_children = emit && display_type != 0;
            for _ in 0..2 {
                slot_display(wrapper, connection, target, mappings, emit_children)?;
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
/// Direct item IDs use Via's identity fallback when there is no mapping row.
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

    let mut mapped = Vec::with_capacity((selector - 1) as usize);
    for _ in 0..(selector - 1) {
        let id = wrapper.read(&VAR_INT)?.0;
        mapped.push(mapped_id_or_identity(items, id));
    }
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
        BOOL.write(&mut payload, &true).unwrap(); // replace
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
        assert!(added.iter().any(|component| {
            component.id == i32::from(DataComponent::AttackAnimation.to_id())
        }));

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
            STRING.read(&mut cursor).unwrap().as_ref(),
            "minecraft:stone_crafting_materials"
        );
        assert_eq!(U8.read(&mut cursor).unwrap(), 3);
        assert!(BOOL.read(&mut cursor).unwrap(), "replace flag is preserved");
        assert!(cursor.is_empty());
        assert_eq!(
            translated.packet.to_id(TARGET),
            RECIPE_BOOK_ADD.to_id(TARGET)
        );
        remove_connection(key);
    }

    #[test]
    fn recipe_book_add_keeps_the_trailing_replace_flag() {
        let key = 0x2623_0010;
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap();
        BOOL.write(&mut payload, &false).unwrap();
        let translated = translate_clientbound(key, TARGET, PLAY, RECIPE_BOOK_ADD.v26_3, &payload)
            .expect("empty recipe book update is still translated");
        let mut read = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(0));
        assert!(!BOOL.read(&mut read).unwrap());
        assert!(read.is_empty());
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
            STRING.read(&mut cursor).unwrap().as_ref(),
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
    fn item_ids_without_mapping_rows_follow_vias_identity_fallback() {
        let key = 0x2623_0004;
        let payload = recipe_payload(70_000);
        let translated = translate_clientbound(key, TARGET, PLAY, RECIPE_BOOK_ADD.v26_3, &payload)
            .expect("an item without an explicit mapping should not invalidate the packet");
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
                VarInt(SLOT_DISPLAY_ITEM)
            );
            assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(70_000));
        }
        // The item-stack display becomes empty because the whole stack cannot
        // be mapped by StructuredItemRewriter.
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap(),
            VarInt(SLOT_DISPLAY_EMPTY)
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap(),
            VarInt(SLOT_DISPLAY_ITEM)
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(70_000));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(3));
        assert!(BOOL.read(&mut cursor).unwrap());
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2)); // one direct ID
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(70_000));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(0));
        assert_eq!(
            STRING.read(&mut cursor).unwrap().as_ref(),
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

    #[test]
    fn update_recipes_rewrites_item_arrays_holders_and_stonecutter_displays() {
        let key = 0x2623_0019;
        let mappings = MappingData::get().composed(TARGET);
        let (source_item, target_item) = mapped_source_item(mappings);
        let unmapped_item = 70_000;
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // recipe groups
        STRING
            .write(&mut payload, &"minecraft:building_blocks".into())
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(2)).unwrap(); // item ID array
        push_var_int(&mut payload, source_item);
        push_var_int(&mut payload, unmapped_item);
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // stonecutter recipes
        VAR_INT.write(&mut payload, &VarInt(3)).unwrap(); // holder: two IDs
        push_var_int(&mut payload, source_item);
        push_var_int(&mut payload, unmapped_item);
        push_item_display(&mut payload, source_item);

        let translated = translate_clientbound(
            key,
            TARGET,
            PLAY,
            crate::packet::mappings::clientbound::play::UPDATE_RECIPES.v26_3,
            &payload,
        )
        .expect("26.2 update-recipes payload should translate");
        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(
            STRING.read(&mut cursor).unwrap().as_ref(),
            "minecraft:building_blocks"
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(2));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(unmapped_item));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(1));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(3));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(unmapped_item));
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ITEM).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert!(cursor.is_empty());
        remove_connection(key);
    }

    #[test]
    fn place_ghost_recipe_rewrites_container_and_recipe_display() {
        let key = 0x2623_0020;
        let target = V::V_1_21_11;
        let mappings = MappingData::get().composed(target);
        let source_item = i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id);
        let target_item = mapped_id(&mappings.items, source_item).unwrap();
        let mut payload = Vec::new();
        push_var_int(&mut payload, 9); // container ID
        push_var_int(&mut payload, 3); // stonecutter display
        for _ in 0..3 {
            push_item_display(&mut payload, source_item);
        }

        let translated = translate_clientbound(
            key,
            target,
            PLAY,
            crate::packet::mappings::clientbound::play::PLACE_GHOST_RECIPE.v26_3,
            &payload,
        )
        .expect("1.21.11 ghost-recipe payload should translate");
        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(9));
        assert_eq!(VAR_INT.read(&mut cursor).unwrap(), VarInt(3));
        for _ in 0..3 {
            assert_eq!(
                VAR_INT.read(&mut cursor).unwrap().0,
                mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ITEM).unwrap()
            );
            assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        }
        assert!(cursor.is_empty());
        remove_connection(key);
    }

    #[test]
    fn update_and_ghost_recipe_packets_are_dropped_before_recipe_display_support() {
        let key = 0x2623_0021;
        let target = V::V_1_20_5;
        assert!(
            translate_clientbound(
                key,
                target,
                PLAY,
                crate::packet::mappings::clientbound::play::UPDATE_RECIPES.v26_3,
                &[0],
            )
            .is_none()
        );
        assert!(
            translate_clientbound(
                key,
                target,
                PLAY,
                crate::packet::mappings::clientbound::play::PLACE_GHOST_RECIPE.v26_3,
                &[0],
            )
            .is_none()
        );
        remove_connection(key);
    }

    #[test]
    fn update_and_ghost_recipe_handlers_reject_non_26_3_source_layouts() {
        let key = 0x2623_0022;
        let ctx = Ctx {
            step: crate::api::Step {
                from: V::V_26_3,
                to: V::V_26_2,
            },
            mappings: MappingData::get().step(V::V_26_3),
            layout: V::V_26_2,
        };
        let mut connection = UserConnection::new(key, TARGET);
        let mut update_wrapper = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::UPDATE_RECIPES,
            &[],
        );
        assert!(matches!(
            rewrite_update_recipes(&mut update_wrapper, &mut connection, &ctx),
            Err(TranslateError::Unsupported("update recipes source layout"))
        ));

        let mut ghost_wrapper = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::PLACE_GHOST_RECIPE,
            &[],
        );
        assert!(matches!(
            rewrite_place_ghost_recipe(&mut ghost_wrapper, &mut connection, &ctx),
            Err(TranslateError::Unsupported("ghost recipe source layout"))
        ));
        remove_connection(key);
    }

    fn translate_slot_display(payload: &[u8], target: V, key: u64) -> Vec<u8> {
        let mappings = MappingData::get().composed(target);
        let mut wrapper = PacketWrapper::new(&RECIPE_BOOK_ADD, payload);
        let mut connection = UserConnection::new(key, target);
        slot_display(&mut wrapper, &mut connection, target, mappings, true).unwrap();
        let translated = wrapper.finish().unwrap().unwrap().payload;
        remove_connection(key);
        translated
    }

    fn push_var_int(payload: &mut Vec<u8>, value: i32) {
        VAR_INT.write(payload, &VarInt(value)).unwrap();
    }

    fn push_item_display(payload: &mut Vec<u8>, item_id: i32) {
        push_var_int(payload, SLOT_DISPLAY_ITEM);
        push_var_int(payload, item_id);
    }

    fn push_empty_display(payload: &mut Vec<u8>) {
        push_var_int(payload, SLOT_DISPLAY_EMPTY);
    }

    #[test]
    fn with_any_potion_rewrites_its_nested_display() {
        let key = 0x2623_0010;
        let mappings = MappingData::get().composed(TARGET);
        let (source_item, target_item) = mapped_source_item(mappings);
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_WITH_ANY_POTION);
        push_item_display(&mut payload, source_item);

        let translated = translate_slot_display(&payload, TARGET, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_WITH_ANY_POTION).unwrap()
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ITEM).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert!(cursor.is_empty());
    }

    #[test]
    fn only_with_component_rewrites_nested_item_and_component_ids() {
        let key = 0x2623_0011;
        let mappings = MappingData::get().composed(TARGET);
        let (source_item, target_item) = mapped_source_item(mappings);
        let source_component = i32::from(DataComponent::Damage.to_id());
        let target_component = mapped_id(&mappings.data_component_type, source_component).unwrap();
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_ONLY_WITH_COMPONENT);
        push_item_display(&mut payload, source_item);
        push_var_int(&mut payload, source_component);

        let translated = translate_slot_display(&payload, TARGET, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ONLY_WITH_COMPONENT).unwrap()
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ITEM).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_component);
        assert!(cursor.is_empty());
    }

    #[test]
    fn only_with_component_uses_empty_component_id_when_target_lacks_it() {
        let key = 0x2623_0018;
        let mappings = MappingData::get().composed(TARGET);
        let missing_component = i32::from(DataComponent::CushionColor.to_id());
        assert!(mapped_id(&mappings.data_component_type, missing_component).is_none());
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_ONLY_WITH_COMPONENT);
        push_empty_display(&mut payload);
        push_var_int(&mut payload, missing_component);

        let translated = translate_slot_display(&payload, TARGET, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ONLY_WITH_COMPONENT).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, SLOT_DISPLAY_EMPTY);
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 0);
        assert!(cursor.is_empty());
    }

    #[test]
    fn tag_holder_sets_downgrade_to_string_and_direct_ids_use_the_via_placeholder() {
        let target = V::V_1_21_11;
        let key = 0x2623_0012;
        let mappings = MappingData::get().composed(target);
        let mut named_payload = Vec::new();
        push_var_int(&mut named_payload, SLOT_DISPLAY_TAG);
        push_var_int(&mut named_payload, 0); // HolderSet tag selector
        STRING
            .write(
                &mut named_payload,
                &"minecraft:stone_crafting_materials".into(),
            )
            .unwrap();

        let translated = translate_slot_display(&named_payload, target, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_TAG).unwrap()
        );
        assert_eq!(
            STRING.read(&mut cursor).unwrap().as_ref(),
            "minecraft:stone_crafting_materials"
        );
        assert!(cursor.is_empty());

        let key = 0x2623_0013;
        let mut ids_payload = Vec::new();
        push_var_int(&mut ids_payload, SLOT_DISPLAY_TAG);
        push_var_int(&mut ids_payload, 2); // one explicit item id plus selector
        push_var_int(&mut ids_payload, 1_234);
        let translated = translate_slot_display(&ids_payload, target, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_TAG).unwrap()
        );
        assert_eq!(STRING.read(&mut cursor).unwrap().as_ref(), "planks");
        assert!(cursor.is_empty());
    }

    #[test]
    fn dyed_rewrites_both_nested_displays() {
        let key = 0x2623_0014;
        let mappings = MappingData::get().composed(TARGET);
        let (source_item, target_item) = mapped_source_item(mappings);
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_DYED);
        push_item_display(&mut payload, source_item);
        push_var_int(&mut payload, SLOT_DISPLAY_ANY_FUEL);

        let translated = translate_slot_display(&payload, TARGET, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_DYED).unwrap()
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ITEM).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ANY_FUEL).unwrap()
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn smithing_trim_rewrites_base_and_material_and_preserves_pattern_payload() {
        let key = 0x2623_0015;
        let target = V::V_1_21_11;
        let mappings = MappingData::get().composed(target);
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_SMITHING_TRIM);
        push_empty_display(&mut payload);
        push_var_int(&mut payload, SLOT_DISPLAY_ANY_FUEL);
        STRING
            .write(&mut payload, &"minecraft:spire".into())
            .unwrap();
        let description = Some(pumpkin_nbt::tag::NbtTag::String("trim description".into()));
        NbtT::for_version(V::V_26_3)
            .write(&mut payload, &description)
            .unwrap();
        BOOL.write(&mut payload, &true).unwrap();

        let translated = translate_slot_display(&payload, target, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_SMITHING_TRIM).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, SLOT_DISPLAY_EMPTY);
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ANY_FUEL).unwrap()
        );
        assert_eq!(
            STRING.read(&mut cursor).unwrap().as_ref(),
            "minecraft:spire"
        );
        assert_eq!(
            NbtT::for_version(V::V_26_3).read(&mut cursor).unwrap(),
            description
        );
        assert!(BOOL.read(&mut cursor).unwrap());
        assert!(cursor.is_empty());
    }

    #[test]
    fn with_remainder_rewrites_input_and_remainder_displays() {
        let key = 0x2623_0016;
        let target = V::V_1_21_11;
        let mappings = MappingData::get().composed(target);
        let (source_item, target_item) = mapped_source_item(mappings);
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_WITH_REMAINDER);
        push_item_display(&mut payload, source_item);
        push_empty_display(&mut payload);

        let translated = translate_slot_display(&payload, target, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_WITH_REMAINDER).unwrap()
        );
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_ITEM).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, target_item);
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, SLOT_DISPLAY_EMPTY);
        assert!(cursor.is_empty());
    }

    #[test]
    fn unsupported_nested_slot_display_is_dropped_without_leaving_payload_bytes() {
        let key = 0x2623_0017;
        let target = V::V_1_21_11;
        let mappings = MappingData::get().composed(target);
        let source_item = i32::from(pumpkin_data::item::Item::DIAMOND_SWORD.id);
        let mut payload = Vec::new();
        push_var_int(&mut payload, SLOT_DISPLAY_DYED);
        push_item_display(&mut payload, source_item);
        push_item_display(&mut payload, source_item);

        let translated = translate_slot_display(&payload, target, key);
        let mut cursor = translated.as_slice();
        assert_eq!(
            VAR_INT.read(&mut cursor).unwrap().0,
            mapped_id(&mappings.slot_displays, SLOT_DISPLAY_EMPTY).unwrap()
        );
        assert!(cursor.is_empty());
    }
}
