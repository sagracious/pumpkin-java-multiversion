//! Best-effort bridge from 26.3 recipe displays to pre-1.21.2 recipe packets.
//!
//! This follows ViaBackwards 5.12.0's RecipeStorage model: old clients receive
//! serializer definitions reconstructed from recipes the server has unlocked,
//! followed by a legacy recipe-book init/remove packet. The source protocol does
//! not expose every legacy recipe field, so this intentionally remains lossy:
//! smithing serializers are consumed but omitted; named tags and visual-only
//! slot displays become empty alternatives; composites keep only the first
//! child; newer book categories are folded into older categories. Unknown
//! codecs or malformed payloads fail closed. Stonecutter entries have no source
//! display IDs, so their synthesized IDs cannot be matched exactly by removals.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::rewriter::item::StructuredItemRewriter;
use crate::api::rewriter::item_backup::backup_clientbound_item;
use crate::api::types::{
    BOOL, F32T, I8, Item, ItemT, NbtT, STRING, TEMPLATE_ITEM, U8, VAR_INT, WireType,
};
use crate::api::{MappingData, PacketWrapper, TranslateError, UserConnection};
use crate::data::mappings::ComposedMappings;
use crate::packet::mappings::clientbound::play::{UNLOCK_RECIPES, UPDATE_RECIPES};

const MAX_RECIPE_ENTRIES: i32 = 16_384;
const MAX_RECIPE_LIST: i32 = 4_096;
const RECIPE_BOOK_SETTINGS: usize = 8;

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

#[derive(Clone, Debug)]
enum RecipeData {
    Shapeless {
        ingredients: Vec<Vec<Item>>,
        result: Item,
    },
    Shaped {
        width: i32,
        height: i32,
        ingredients: Vec<Vec<Item>>,
        result: Item,
    },
    Furnace {
        ingredient: Vec<Item>,
        result: Item,
        duration: i32,
        experience: f32,
    },
    Stonecutter {
        ingredient: Vec<Item>,
        result: Item,
    },
}

#[derive(Clone, Debug)]
struct Recipe {
    id: i32,
    group: Option<i32>,
    category: i32,
    highlight: bool,
    data: RecipeData,
}

#[derive(Clone, Debug)]
struct RecipeBookStorage {
    recipes: Vec<Recipe>,
    stonecutter_recipes: Vec<Recipe>,
    settings: [bool; RECIPE_BOOK_SETTINGS],
}

impl Default for RecipeBookStorage {
    fn default() -> Self {
        Self {
            recipes: Vec::new(),
            stonecutter_recipes: Vec::new(),
            settings: [false; RECIPE_BOOK_SETTINGS],
        }
    }
}

fn storage_mut(connection: &mut UserConnection) -> &mut RecipeBookStorage {
    if connection.get::<RecipeBookStorage>().is_none() {
        connection.put(RecipeBookStorage::default());
    }
    connection
        .get_mut::<RecipeBookStorage>()
        .expect("recipe book storage was just inserted")
}

pub(super) fn rewrite_recipe_book_add(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
) -> Result<(), TranslateError> {
    let mappings = MappingData::get().composed(target);
    let entry_count = read_count(wrapper, MAX_RECIPE_ENTRIES, "recipe entry count")?;
    let mut additions = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        if let Some(recipe) = read_recipe(wrapper, connection, target, mappings)? {
            additions.push(recipe);
        }
    }
    let replace = wrapper.read(&BOOL)?;

    let storage = storage_mut(connection);
    if replace {
        storage.recipes.clear();
    }
    storage.recipes.extend(additions);

    // PacketWrapper's extra packets are not run through later protocol steps,
    // and host adapters do not all place extras on the same side of the main
    // packet. Emit both translations as ordered extras so UPDATE_RECIPES always
    // precedes the legacy init packet, with both payloads already final.
    let update = encode_update_recipes(storage, target, mappings)?;
    let unlock = encode_recipe_book_init(storage)?;
    wrapper.send_extra(&UPDATE_RECIPES, update);
    wrapper.send_extra(&UNLOCK_RECIPES, unlock);
    wrapper.cancel();
    Ok(())
}

pub(super) fn rewrite_update_recipes(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
) -> Result<(), TranslateError> {
    let mappings = MappingData::get().composed(target);
    let groups = read_count(wrapper, MAX_RECIPE_ENTRIES, "recipe group count")?;
    for _ in 0..groups {
        wrapper.read(&STRING)?; // Group names are not enough to rebuild recipes.
        let count = read_count(wrapper, MAX_RECIPE_LIST, "recipe group item count")?;
        for _ in 0..count {
            wrapper.read(&VAR_INT)?;
        }
    }

    let count = read_count(wrapper, MAX_RECIPE_ENTRIES, "stonecutter recipe count")?;
    let mut stonecutters = Vec::with_capacity(count);
    for index in 0..count {
        let ingredient = read_holder_set_items(wrapper, connection, target, mappings, true)?;
        let result = read_single_slot_display(wrapper, connection, target, mappings)?;
        stonecutters.push(Recipe {
            // UPDATE_RECIPES does not carry the source recipe display IDs.
            // ViaBackwards assigns these after the highest ordinary recipe ID.
            id: i32::try_from(index)
                .map_err(|_| TranslateError::Unsupported("stonecutter recipe identifier"))?,
            group: None,
            category: 0,
            highlight: false,
            data: RecipeData::Stonecutter { ingredient, result },
        });
    }
    storage_mut(connection).stonecutter_recipes = stonecutters;
    wrapper.cancel();
    Ok(())
}

pub(super) fn rewrite_recipe_book_remove(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
) -> Result<(), TranslateError> {
    if connection.version >= V::V_1_21_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    if layout != V::V_26_3 {
        return Err(TranslateError::Unsupported("recipe remove source layout"));
    }

    let count = read_count(wrapper, MAX_RECIPE_ENTRIES, "recipe remove count")?;
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id = wrapper.read(&VAR_INT)?.0;
        if id < 0 {
            return Err(TranslateError::Unsupported("recipe remove identifier"));
        }
        ids.push(id);
    }

    let storage = storage_mut(connection);
    storage.recipes.retain(|recipe| !ids.contains(&recipe.id));
    let payload = encode_recipe_book_remove(storage, &ids)?;
    wrapper.set_packet(&UNLOCK_RECIPES);
    wrapper.replace_remaining(payload);
    Ok(())
}

pub(super) fn rewrite_recipe_book_settings(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: V,
) -> Result<(), TranslateError> {
    if connection.version >= V::V_1_21_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    if layout != V::V_26_3 {
        return Err(TranslateError::Unsupported("recipe settings source layout"));
    }

    let mut settings = [false; RECIPE_BOOK_SETTINGS];
    for setting in &mut settings {
        *setting = wrapper.read(&BOOL)?;
    }
    storage_mut(connection).settings = settings;
    wrapper.cancel();
    Ok(())
}

pub(super) fn rewrite_place_recipe(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
) -> Result<(), TranslateError> {
    if connection.version >= V::V_1_21_2 {
        wrapper.passthrough_all();
        return Ok(());
    }

    // 1.21.1 and older send a signed-byte container id plus a string recipe
    // key. The 26.3 server expects a VarInt container and numeric display id.
    let container = i32::from(wrapper.read(&I8)?);
    let recipe = parse_synthetic_recipe_id(&wrapper.read(&STRING)?)?;
    write_var_int(wrapper, container)?;
    write_var_int(wrapper, recipe)?;
    wrapper.passthrough_all();
    Ok(())
}

pub(super) fn rewrite_seen_recipe(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
) -> Result<(), TranslateError> {
    if connection.version >= V::V_1_21_2 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let recipe = parse_synthetic_recipe_id(&wrapper.read(&STRING)?)?;
    write_var_int(wrapper, recipe)?;
    wrapper.passthrough_all();
    Ok(())
}

fn parse_synthetic_recipe_id(identifier: &str) -> Result<i32, TranslateError> {
    let numeric = identifier.strip_prefix("minecraft:").unwrap_or(identifier);
    let id = numeric
        .parse::<i32>()
        .map_err(|_| TranslateError::Unsupported("synthetic recipe identifier"))?;
    if id < 0 {
        return Err(TranslateError::Unsupported("synthetic recipe identifier"));
    }
    Ok(id)
}

fn read_recipe(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Result<Option<Recipe>, TranslateError> {
    let id = wrapper.read(&VAR_INT)?.0;
    if id < 0 {
        return Err(TranslateError::Unsupported("recipe display identifier"));
    }
    let display_type = wrapper.read(&VAR_INT)?.0;
    let data = match display_type {
        0 => {
            let ingredients = read_slot_display_list(wrapper, connection, target, mappings)?;
            let result = read_single_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?; // Crafting station.
            Some(RecipeData::Shapeless {
                ingredients,
                result,
            })
        }
        1 => {
            let width = wrapper.read(&VAR_INT)?.0;
            let height = wrapper.read(&VAR_INT)?.0;
            if !(1..=3).contains(&width) || !(1..=3).contains(&height) {
                return Err(TranslateError::Unsupported("shaped recipe dimensions"));
            }
            let ingredients = read_slot_display_list(wrapper, connection, target, mappings)?;
            if ingredients.len() != (width * height) as usize {
                return Err(TranslateError::Unsupported(
                    "shaped recipe ingredient count",
                ));
            }
            let result = read_single_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?; // Crafting station.
            Some(RecipeData::Shaped {
                width,
                height,
                ingredients,
                result,
            })
        }
        2 => {
            let ingredient = read_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?; // Fuel.
            let result = read_single_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?; // Crafting station.
            let duration = wrapper.read(&VAR_INT)?.0;
            let experience = wrapper.read(&F32T)?;
            Some(RecipeData::Furnace {
                ingredient,
                result,
                duration,
                experience,
            })
        }
        3 => {
            read_slot_display(wrapper, connection, target, mappings)?; // Stonecutter input.
            read_slot_display(wrapper, connection, target, mappings)?; // Stonecutter result.
            read_slot_display(wrapper, connection, target, mappings)?; // Station.
            None // Full stonecutter data comes from UPDATE_RECIPES.
        }
        4 => {
            for _ in 0..5 {
                read_slot_display(wrapper, connection, target, mappings)?;
            }
            None // ViaBackwards cannot reconstruct legacy smithing serializers.
        }
        _ => return Err(TranslateError::Unsupported("recipe display type")),
    };

    let encoded_group = wrapper.read(&VAR_INT)?.0;
    if encoded_group < 0 {
        return Err(TranslateError::Unsupported("recipe group identifier"));
    }
    let group = (encoded_group != 0).then_some(encoded_group - 1);
    let category = wrapper.read(&VAR_INT)?.0;
    if category < 0 {
        return Err(TranslateError::Unsupported("recipe book category"));
    }
    if wrapper.read(&BOOL)? {
        let count = read_count(wrapper, MAX_RECIPE_LIST, "crafting requirement count")?;
        for _ in 0..count {
            read_holder_set_items(wrapper, connection, target, mappings, false)?;
        }
    }
    let flags = wrapper.read(&U8)?;

    Ok(data.map(|data| Recipe {
        id,
        group,
        category,
        highlight: flags & 0x02 != 0,
        data,
    }))
}

fn read_slot_display_list(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Result<Vec<Vec<Item>>, TranslateError> {
    let count = read_count(wrapper, MAX_RECIPE_LIST, "recipe slot display count")?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(read_slot_display(wrapper, connection, target, mappings)?);
    }
    Ok(items)
}

fn read_single_slot_display(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Result<Item, TranslateError> {
    let items = read_slot_display(wrapper, connection, target, mappings)?;
    Ok(items
        .into_iter()
        .next()
        .unwrap_or_else(|| placeholder_item(target, mappings)))
}

fn read_slot_display(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Result<Vec<Item>, TranslateError> {
    let kind = wrapper.read(&VAR_INT)?.0;
    match kind {
        SLOT_DISPLAY_EMPTY | SLOT_DISPLAY_ANY_FUEL => Ok(Vec::new()),
        SLOT_DISPLAY_WITH_ANY_POTION => {
            read_slot_display(wrapper, connection, target, mappings)?;
            Ok(Vec::new())
        }
        SLOT_DISPLAY_ONLY_WITH_COMPONENT => {
            read_slot_display(wrapper, connection, target, mappings)?;
            if wrapper.read(&VAR_INT)?.0 < 0 {
                return Err(TranslateError::Unsupported("slot display component"));
            }
            Ok(Vec::new())
        }
        SLOT_DISPLAY_ITEM => {
            let id = wrapper.read(&VAR_INT)?.0;
            if id < 0 {
                return Err(TranslateError::Unsupported("recipe slot item id"));
            }
            Ok(lower_source_item(id, connection, target, mappings))
        }
        SLOT_DISPLAY_ITEM_STACK => {
            let native = wrapper.read(&TEMPLATE_ITEM)?;
            let mapped = lower_item(&native, connection, target, mappings);
            Ok((!mapped.is_empty()).then_some(mapped).into_iter().collect())
        }
        SLOT_DISPLAY_TAG => {
            let selector = wrapper.read(&VAR_INT)?.0;
            if selector == 0 {
                wrapper.read(&STRING)?; // A named tag cannot be represented by the old item array.
                Ok(Vec::new())
            } else {
                let count = selector
                    .checked_sub(1)
                    .filter(|count| *count >= 0 && *count <= MAX_RECIPE_LIST)
                    .ok_or(TranslateError::Unsupported("recipe slot tag set"))?;
                let mut items = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let id = wrapper.read(&VAR_INT)?.0;
                    if id < 0 {
                        return Err(TranslateError::Unsupported("recipe slot tag item"));
                    }
                    items.extend(lower_source_item(id, connection, target, mappings));
                }
                Ok(items)
            }
        }
        SLOT_DISPLAY_DYED => {
            read_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?;
            Ok(Vec::new())
        }
        SLOT_DISPLAY_SMITHING_TRIM => {
            read_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?;
            wrapper.read(&STRING)?; // Pattern asset name.
            wrapper.read(&NbtT::for_version(V::V_26_3))?;
            wrapper.read(&BOOL)?; // Decal flag.
            Ok(Vec::new())
        }
        SLOT_DISPLAY_WITH_REMAINDER => {
            read_slot_display(wrapper, connection, target, mappings)?;
            read_slot_display(wrapper, connection, target, mappings)?;
            Ok(Vec::new())
        }
        SLOT_DISPLAY_COMPOSITE => {
            let count = read_count(wrapper, MAX_RECIPE_LIST, "composite slot display count")?;
            let mut first = Vec::new();
            for index in 0..count {
                let items = read_slot_display(wrapper, connection, target, mappings)?;
                if index == 0 {
                    first = items;
                }
            }
            Ok(first)
        }
        _ => Err(TranslateError::Unsupported("recipe slot display codec")),
    }
}

fn read_holder_set_items(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
    stonecutter_input: bool,
) -> Result<Vec<Item>, TranslateError> {
    let selector = wrapper.read(&VAR_INT)?.0;
    if selector == 0 {
        wrapper.read(&STRING)?; // Named tags need server tag data unavailable here.
        return Ok(if stonecutter_input {
            vec![placeholder_item(target, mappings)]
        } else {
            Vec::new()
        });
    }
    let count = selector
        .checked_sub(1)
        .filter(|count| *count >= 0 && *count <= MAX_RECIPE_LIST)
        .ok_or(TranslateError::Unsupported("recipe holder set"))?;
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let id = wrapper.read(&VAR_INT)?.0;
        if id < 0 {
            return Err(TranslateError::Unsupported("recipe holder item id"));
        }
        items.extend(lower_source_item(id, connection, target, mappings));
    }
    Ok(items)
}

fn lower_source_item(
    id: i32,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Vec<Item> {
    let native = Item::Structured {
        count: 1,
        id,
        added: Vec::new(),
        removed: Vec::new(),
    };
    let mapped = lower_item(&native, connection, target, mappings);
    if mapped.is_empty() {
        Vec::new()
    } else {
        vec![mapped]
    }
}

fn lower_item(
    native: &Item,
    connection: &mut UserConnection,
    target: V,
    mappings: &ComposedMappings,
) -> Item {
    let mut mapped = StructuredItemRewriter::to_version(native, target, mappings);
    backup_clientbound_item(connection, native, &mut mapped, target, mappings);
    mapped
}

fn placeholder_item(target: V, mappings: &ComposedMappings) -> Item {
    let native = Item::Structured {
        count: 1,
        id: 1,
        added: Vec::new(),
        removed: Vec::new(),
    };
    StructuredItemRewriter::to_version(&native, target, mappings)
}

fn encode_update_recipes(
    storage: &RecipeBookStorage,
    target: V,
    mappings: &ComposedMappings,
) -> Result<Vec<u8>, TranslateError> {
    let mut recipes = storage.recipes.clone();
    let mut highest_id = recipes.iter().map(|recipe| recipe.id).max().unwrap_or(-1);
    for stonecutter in &storage.stonecutter_recipes {
        highest_id = highest_id
            .checked_add(1)
            .ok_or(TranslateError::Unsupported(
                "stonecutter synthetic identifier",
            ))?;
        let mut recipe = stonecutter.clone();
        recipe.id = highest_id;
        recipes.push(recipe);
    }
    recipes.sort_by_key(|recipe| recipe.id);

    let mut payload = Vec::new();
    write_count(&mut payload, recipes.len())?;
    for recipe in &recipes {
        write_recipe_definition(&mut payload, recipe, target, mappings)?;
    }
    Ok(payload)
}

fn write_recipe_definition(
    payload: &mut Vec<u8>,
    recipe: &Recipe,
    target: V,
    mappings: &ComposedMappings,
) -> Result<(), TranslateError> {
    let identifier = recipe_identifier(recipe.id);
    if target >= V::V_1_20_5 {
        write_string(payload, &identifier)?;
        let serializer = source_serializer_id(recipe)?;
        let mapped = mappings
            .recipe_serializers
            .map(serializer)
            .and_then(|id| i32::try_from(id).ok())
            .ok_or(TranslateError::Unsupported("recipe serializer mapping"))?;
        write_var_int_to(payload, mapped)?;
    } else {
        write_string(payload, source_serializer_name(recipe)?)?;
        write_string(payload, &identifier)?;
    }

    let group = recipe
        .group
        .map_or_else(String::new, |group| group.to_string());
    match &recipe.data {
        RecipeData::Shapeless {
            ingredients,
            result,
        } => {
            write_string(payload, &group)?;
            if target >= V::V_1_19_3 {
                write_var_int_to(payload, legacy_category(recipe.category))?;
            }
            write_ingredients(payload, ingredients, target)?;
            write_item(payload, result, target)?;
        }
        RecipeData::Shaped {
            width,
            height,
            ingredients,
            result,
        } => {
            if target >= V::V_1_20_3 {
                write_string(payload, &group)?;
                if target >= V::V_1_19_3 {
                    write_var_int_to(payload, legacy_category(recipe.category))?;
                }
                write_var_int_to(payload, *width)?;
                write_var_int_to(payload, *height)?;
            } else {
                write_var_int_to(payload, *width)?;
                write_var_int_to(payload, *height)?;
                write_string(payload, &group)?;
                if target >= V::V_1_19_3 {
                    write_var_int_to(payload, legacy_category(recipe.category))?;
                }
            }
            for ingredient in ingredients {
                write_items(payload, ingredient, target)?;
            }
            write_item(payload, result, target)?;
            if target >= V::V_1_19_4 {
                BOOL.write(payload, &false)?; // Show notification.
            }
        }
        RecipeData::Furnace {
            ingredient,
            result,
            duration,
            experience,
        } => {
            write_string(payload, &group)?;
            if target >= V::V_1_19_3 {
                write_var_int_to(payload, legacy_category(recipe.category))?;
            }
            write_items(payload, ingredient, target)?;
            write_item(payload, result, target)?;
            F32T.write(payload, experience)?;
            write_var_int_to(payload, *duration)?;
        }
        RecipeData::Stonecutter { ingredient, result } => {
            write_string(payload, &group)?;
            write_items(payload, ingredient, target)?;
            write_item(payload, result, target)?;
        }
    }
    Ok(())
}

fn source_serializer_id(recipe: &Recipe) -> Result<u32, TranslateError> {
    let id = match &recipe.data {
        RecipeData::Shaped { .. } => 0,
        RecipeData::Shapeless { .. } => 1,
        RecipeData::Furnace { .. } => match recipe.category {
            7 | 8 => 16,
            9 => 17,
            12 => 18,
            _ => 15,
        },
        RecipeData::Stonecutter { .. } => 19,
    };
    u32::try_from(id).map_err(|_| TranslateError::Unsupported("recipe serializer identifier"))
}

fn source_serializer_name(recipe: &Recipe) -> Result<&'static str, TranslateError> {
    Ok(match &recipe.data {
        RecipeData::Shaped { .. } => "minecraft:crafting_shaped",
        RecipeData::Shapeless { .. } => "minecraft:crafting_shapeless",
        RecipeData::Furnace { .. } => match recipe.category {
            7 | 8 => "minecraft:blasting",
            9 => "minecraft:smoking",
            12 => "minecraft:campfire_cooking",
            _ => "minecraft:smelting",
        },
        RecipeData::Stonecutter { .. } => "minecraft:stonecutting",
    })
}

fn encode_recipe_book_init(storage: &RecipeBookStorage) -> Result<Vec<u8>, TranslateError> {
    let mut recipes = storage.recipes.clone();
    let mut highest_id = recipes.iter().map(|recipe| recipe.id).max().unwrap_or(-1);
    for stonecutter in &storage.stonecutter_recipes {
        highest_id = highest_id
            .checked_add(1)
            .ok_or(TranslateError::Unsupported(
                "stonecutter synthetic identifier",
            ))?;
        let mut recipe = stonecutter.clone();
        recipe.id = highest_id;
        recipes.push(recipe);
    }
    recipes.sort_by_key(|recipe| recipe.id);

    let mut payload = Vec::new();
    write_var_int_to(&mut payload, 0)?; // Legacy recipe-book init.
    for setting in &storage.settings {
        BOOL.write(&mut payload, setting)?;
    }
    write_count(&mut payload, recipes.len())?;
    for recipe in &recipes {
        write_string(&mut payload, &recipe_identifier(recipe.id))?;
    }
    let highlights: Vec<_> = recipes.iter().filter(|recipe| recipe.highlight).collect();
    write_count(&mut payload, highlights.len())?;
    for recipe in highlights {
        write_string(&mut payload, &recipe_identifier(recipe.id))?;
    }
    Ok(payload)
}

fn encode_recipe_book_remove(
    storage: &RecipeBookStorage,
    ids: &[i32],
) -> Result<Vec<u8>, TranslateError> {
    let mut payload = Vec::new();
    write_var_int_to(&mut payload, 2)?; // Legacy remove action.
    for setting in &storage.settings {
        BOOL.write(&mut payload, setting)?;
    }
    write_count(&mut payload, ids.len())?;
    for id in ids {
        write_string(&mut payload, &recipe_identifier(*id))?;
    }
    Ok(payload)
}

fn write_ingredients(
    payload: &mut Vec<u8>,
    ingredients: &[Vec<Item>],
    target: V,
) -> Result<(), TranslateError> {
    write_count(payload, ingredients.len())?;
    for ingredient in ingredients {
        write_items(payload, ingredient, target)?;
    }
    Ok(())
}

fn write_items(payload: &mut Vec<u8>, items: &[Item], target: V) -> Result<(), TranslateError> {
    let items: Vec<_> = items.iter().filter(|item| !item.is_empty()).collect();
    write_count(payload, items.len())?;
    let item_type = ItemT::for_version(target);
    for item in items {
        item_type.write(payload, item)?;
    }
    Ok(())
}

fn write_item(payload: &mut Vec<u8>, item: &Item, target: V) -> Result<(), TranslateError> {
    let item = if item.is_empty() {
        placeholder_item(target, MappingData::get().composed(target))
    } else {
        item.clone()
    };
    ItemT::for_version(target).write(payload, &item)?;
    Ok(())
}

fn legacy_category(category: i32) -> i32 {
    // Mirrors ViaBackwards 5.12.0's coarse 1.21 recipe-book categories.
    match category {
        4 | 9 | 12 => 0,             // Food.
        0 | 5 | 7 | 10 => 1,         // Blocks.
        1 | 2 | 3 | 6 | 8 | 11 => 2, // Misc.
        _ => 2,
    }
}

fn recipe_identifier(id: i32) -> String {
    format!("{id:06}")
}

fn read_count(
    wrapper: &mut PacketWrapper,
    maximum: i32,
    what: &'static str,
) -> Result<usize, TranslateError> {
    let count = wrapper.read(&VAR_INT)?.0;
    if !(0..=maximum).contains(&count) {
        return Err(TranslateError::Unsupported(what));
    }
    usize::try_from(count).map_err(|_| TranslateError::Unsupported(what))
}

fn write_count(payload: &mut Vec<u8>, count: usize) -> Result<(), TranslateError> {
    let count =
        i32::try_from(count).map_err(|_| TranslateError::Unsupported("recipe list size"))?;
    write_var_int_to(payload, count)
}

fn write_string(payload: &mut Vec<u8>, value: &str) -> Result<(), TranslateError> {
    STRING.write(payload, &value.to_string().into_boxed_str())?;
    Ok(())
}

fn write_var_int(wrapper: &mut PacketWrapper, value: i32) -> Result<(), TranslateError> {
    wrapper.write(&VAR_INT, &VarInt(value))
}

fn write_var_int_to(payload: &mut Vec<u8>, value: i32) -> Result<(), TranslateError> {
    VAR_INT.write(payload, &VarInt(value))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::mappings::clientbound::play::{
        RECIPE_BOOK_REMOVE, RECIPE_BOOK_SETTINGS as RECIPE_BOOK_SETTINGS_PACKET,
    };
    use crate::packet::mappings::serverbound::play::{PLACE_RECIPE, RECIPE_BOOK_SEEN_RECIPE};

    fn settings_payload(settings: [bool; RECIPE_BOOK_SETTINGS]) -> Vec<u8> {
        let mut payload = Vec::new();
        for setting in settings {
            BOOL.write(&mut payload, &setting).unwrap();
        }
        payload
    }

    fn remove_payload(ids: &[i32]) -> Vec<u8> {
        let mut payload = Vec::new();
        VAR_INT
            .write(&mut payload, &VarInt(ids.len() as i32))
            .unwrap();
        for id in ids {
            VAR_INT.write(&mut payload, &VarInt(*id)).unwrap();
        }
        payload
    }

    fn shapeless_add_payload(item_id: i32, recipe_id: i32, replace: bool) -> Vec<u8> {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // One display entry.
        VAR_INT.write(&mut payload, &VarInt(recipe_id)).unwrap(); // Display id.
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // Shapeless.
        VAR_INT.write(&mut payload, &VarInt(1)).unwrap(); // One ingredient slot.
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_ITEM))
            .unwrap();
        VAR_INT.write(&mut payload, &VarInt(item_id)).unwrap();
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_ITEM))
            .unwrap(); // Result.
        VAR_INT.write(&mut payload, &VarInt(item_id)).unwrap();
        VAR_INT
            .write(&mut payload, &VarInt(SLOT_DISPLAY_EMPTY))
            .unwrap(); // Station.
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // No group.
        VAR_INT.write(&mut payload, &VarInt(3)).unwrap(); // Category.
        BOOL.write(&mut payload, &false).unwrap(); // No crafting requirements.
        U8.write(&mut payload, &2).unwrap(); // Highlight.
        BOOL.write(&mut payload, &replace).unwrap();
        payload
    }

    #[test]
    fn legacy_add_emits_update_before_init_and_honors_replace_and_highlight() {
        let target = V::V_1_16_2;
        let item_id = i32::from(pumpkin_data::item::Item::DIAMOND.id);
        let mut connection = UserConnection::new(0x7a10, target);
        let mut prior = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::RECIPE_BOOK_ADD,
            &shapeless_add_payload(item_id, 8, false),
        );
        rewrite_recipe_book_add(&mut prior, &mut connection, target).unwrap();
        prior.finish_with_outputs().unwrap();

        let mut wrapper = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::RECIPE_BOOK_ADD,
            &shapeless_add_payload(item_id, 7, true),
        );
        rewrite_recipe_book_add(&mut wrapper, &mut connection, target).unwrap();
        let translated = wrapper.finish_with_outputs().unwrap();
        assert!(translated.cancelled);
        assert_eq!(translated.extra.len(), 2);
        assert!(std::ptr::eq(translated.extra[0].0, &UPDATE_RECIPES));
        assert!(std::ptr::eq(translated.extra[1].0, &UNLOCK_RECIPES));

        let mut update = translated.extra[0].1.as_slice();
        assert_eq!(VAR_INT.read(&mut update).unwrap().0, 1);
        assert_eq!(
            STRING.read(&mut update).unwrap().as_ref(),
            "minecraft:crafting_shapeless"
        );
        assert_eq!(STRING.read(&mut update).unwrap().as_ref(), "000007");
        assert_eq!(STRING.read(&mut update).unwrap().as_ref(), ""); // Group.
        assert_eq!(VAR_INT.read(&mut update).unwrap().0, 1); // Ingredient count.
        assert_eq!(VAR_INT.read(&mut update).unwrap().0, 1); // Alternative count.
        let _ingredient = ItemT::for_version(target).read(&mut update).unwrap();
        let _result = ItemT::for_version(target).read(&mut update).unwrap();
        assert!(update.is_empty());

        let mut unlock = translated.extra[1].1.as_slice();
        assert_eq!(VAR_INT.read(&mut unlock).unwrap().0, 0); // Init.
        for _ in 0..RECIPE_BOOK_SETTINGS {
            assert!(!BOOL.read(&mut unlock).unwrap());
        }
        assert_eq!(VAR_INT.read(&mut unlock).unwrap().0, 1);
        assert_eq!(STRING.read(&mut unlock).unwrap().as_ref(), "000007");
        assert_eq!(VAR_INT.read(&mut unlock).unwrap().0, 1);
        assert_eq!(STRING.read(&mut unlock).unwrap().as_ref(), "000007");
        assert!(unlock.is_empty());
    }

    #[test]
    fn legacy_update_recipes_stores_stonecutter_for_the_next_add() {
        let target = V::V_1_16_2;
        let item_id = i32::from(pumpkin_data::item::Item::STONE.id);
        let mut connection = UserConnection::new(0x7a13, target);
        let mut update = Vec::new();
        VAR_INT.write(&mut update, &VarInt(0)).unwrap(); // Ordinary groups.
        VAR_INT.write(&mut update, &VarInt(1)).unwrap(); // Stonecutters.
        VAR_INT.write(&mut update, &VarInt(2)).unwrap(); // HolderSet: one item.
        VAR_INT.write(&mut update, &VarInt(item_id)).unwrap();
        VAR_INT
            .write(&mut update, &VarInt(SLOT_DISPLAY_ITEM))
            .unwrap();
        VAR_INT.write(&mut update, &VarInt(item_id)).unwrap();
        let mut wrapper = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::UPDATE_RECIPES,
            &update,
        );
        rewrite_update_recipes(&mut wrapper, &mut connection, target).unwrap();
        assert!(wrapper.finish_with_outputs().unwrap().cancelled);

        let mut add = Vec::new();
        VAR_INT.write(&mut add, &VarInt(0)).unwrap(); // No unlocked ordinary recipes.
        BOOL.write(&mut add, &true).unwrap(); // Replace ordinary recipes.
        let mut wrapper = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::RECIPE_BOOK_ADD,
            &add,
        );
        rewrite_recipe_book_add(&mut wrapper, &mut connection, target).unwrap();
        let translated = wrapper.finish_with_outputs().unwrap();

        let mut recipes = translated.extra[0].1.as_slice();
        assert_eq!(VAR_INT.read(&mut recipes).unwrap().0, 1);
        assert_eq!(
            STRING.read(&mut recipes).unwrap().as_ref(),
            "minecraft:stonecutting"
        );
        assert_eq!(STRING.read(&mut recipes).unwrap().as_ref(), "000000");
        assert_eq!(STRING.read(&mut recipes).unwrap().as_ref(), "");
        assert_eq!(VAR_INT.read(&mut recipes).unwrap().0, 1); // One ingredient.
        assert_eq!(VAR_INT.read(&mut recipes).unwrap().0, 1); // One alternative.
        let _input = ItemT::for_version(target).read(&mut recipes).unwrap();
        let _result = ItemT::for_version(target).read(&mut recipes).unwrap();
        assert!(recipes.is_empty());

        let mut unlock = translated.extra[1].1.as_slice();
        assert_eq!(VAR_INT.read(&mut unlock).unwrap().0, 0);
        for _ in 0..RECIPE_BOOK_SETTINGS {
            BOOL.read(&mut unlock).unwrap();
        }
        assert_eq!(VAR_INT.read(&mut unlock).unwrap().0, 1);
        assert_eq!(STRING.read(&mut unlock).unwrap().as_ref(), "000000");
    }

    #[test]
    fn settings_are_folded_into_legacy_recipe_book_packets() {
        let target = V::V_1_16_2;
        let mut connection = UserConnection::new(0x7a11, target);
        let settings = [true, false, false, true, true, true, false, false];
        let mut wrapper =
            PacketWrapper::new(&RECIPE_BOOK_SETTINGS_PACKET, &settings_payload(settings));
        rewrite_recipe_book_settings(&mut wrapper, &mut connection, V::V_26_3).unwrap();
        assert!(wrapper.finish_with_outputs().unwrap().cancelled);

        let mut wrapper = PacketWrapper::new(&RECIPE_BOOK_REMOVE, &remove_payload(&[23]));
        rewrite_recipe_book_remove(&mut wrapper, &mut connection, V::V_26_3).unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        assert!(std::ptr::eq(translated.packet, &UNLOCK_RECIPES));
        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 2);
        for expected in settings {
            assert_eq!(BOOL.read(&mut cursor).unwrap(), expected);
        }
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 1);
        assert_eq!(STRING.read(&mut cursor).unwrap().as_ref(), "000023");
        assert!(cursor.is_empty());
    }

    #[test]
    fn old_recipe_clicks_return_the_synthetic_display_id_to_26_3() {
        let target = V::V_1_21;
        let mut connection = UserConnection::new(0x7a12, target);
        let mut payload = Vec::new();
        I8.write(&mut payload, &5).unwrap();
        STRING.write(&mut payload, &"000123".into()).unwrap();
        let mut wrapper = PacketWrapper::new(&PLACE_RECIPE, &payload);
        rewrite_place_recipe(&mut wrapper, &mut connection).unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 5);
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 123);
        assert!(cursor.is_empty());

        let mut payload = Vec::new();
        STRING
            .write(&mut payload, &"minecraft:000123".into())
            .unwrap();
        let mut wrapper = PacketWrapper::new(&RECIPE_BOOK_SEEN_RECIPE, &payload);
        rewrite_seen_recipe(&mut wrapper, &mut connection).unwrap();
        let translated = wrapper.finish().unwrap().unwrap();
        let mut cursor = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 123);
        assert!(cursor.is_empty());
    }

    #[test]
    fn recipe_definition_layout_branches_at_legacy_protocol_changes() {
        let recipe = Recipe {
            id: 7,
            group: Some(2),
            category: 4,
            highlight: true,
            data: RecipeData::Shaped {
                width: 1,
                height: 1,
                ingredients: vec![Vec::new()],
                result: Item::Empty,
            },
        };
        for target in [V::V_1_16_2, V::V_1_19_3, V::V_1_19_4, V::V_1_20_3] {
            let mut payload = Vec::new();
            write_recipe_definition(
                &mut payload,
                &recipe,
                target,
                MappingData::get().composed(target),
            )
            .unwrap();
            let mut cursor = payload.as_slice();
            assert_eq!(
                STRING.read(&mut cursor).unwrap().as_ref(),
                "minecraft:crafting_shaped"
            );
            assert_eq!(STRING.read(&mut cursor).unwrap().as_ref(), "000007");
            if target >= V::V_1_20_3 {
                assert_eq!(STRING.read(&mut cursor).unwrap().as_ref(), "2");
                assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 0); // Category.
                assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 1); // Width.
                assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 1); // Height.
            } else {
                assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 1); // Width.
                assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 1); // Height.
                assert_eq!(STRING.read(&mut cursor).unwrap().as_ref(), "2");
                if target >= V::V_1_19_3 {
                    assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 0); // Category.
                }
            }
            assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 1); // One ingredient slot.
            assert_eq!(VAR_INT.read(&mut cursor).unwrap().0, 0); // No alternatives.
            let _result = ItemT::for_version(target).read(&mut cursor).unwrap();
            if target >= V::V_1_19_4 {
                assert!(!BOOL.read(&mut cursor).unwrap()); // Show notification.
            }
            assert!(cursor.is_empty(), "{target}");
        }

        // Numeric serializer IDs start in 1.20.5; mapping is required for the
        // target registry, while the following recipe payload remains the same.
        for target in [V::V_1_20_5, V::V_1_21] {
            let mut payload = Vec::new();
            write_recipe_definition(
                &mut payload,
                &recipe,
                target,
                MappingData::get().composed(target),
            )
            .unwrap();
            let mut cursor = payload.as_slice();
            assert_eq!(STRING.read(&mut cursor).unwrap().as_ref(), "000007");
            VAR_INT.read(&mut cursor).unwrap(); // Serializer registry ID.
        }
    }

    #[test]
    fn known_smithing_is_consumed_and_unknown_slot_codec_fails_closed() {
        let target = V::V_1_16_2;
        let mappings = MappingData::get().composed(target);
        let mut connection = UserConnection::new(0x7a14, target);

        let mut smithing = Vec::new();
        VAR_INT.write(&mut smithing, &VarInt(3)).unwrap(); // Display id.
        VAR_INT.write(&mut smithing, &VarInt(4)).unwrap(); // Smithing display.
        for _ in 0..5 {
            VAR_INT
                .write(&mut smithing, &VarInt(SLOT_DISPLAY_EMPTY))
                .unwrap();
        }
        VAR_INT.write(&mut smithing, &VarInt(0)).unwrap(); // No group.
        VAR_INT.write(&mut smithing, &VarInt(0)).unwrap(); // Category.
        BOOL.write(&mut smithing, &false).unwrap(); // No requirements.
        U8.write(&mut smithing, &0).unwrap(); // Flags.
        let mut wrapper = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::RECIPE_BOOK_ADD,
            &smithing,
        );
        assert!(
            read_recipe(&mut wrapper, &mut connection, target, mappings)
                .unwrap()
                .is_none()
        );

        let mut unsupported_slot = PacketWrapper::new(
            &crate::packet::mappings::clientbound::play::RECIPE_BOOK_ADD,
            &[99],
        );
        assert!(matches!(
            read_slot_display(&mut unsupported_slot, &mut connection, target, mappings),
            Err(TranslateError::Unsupported("recipe slot display codec"))
        ));
    }
}
