use pumpkin_data::item::Item;
use pumpkin_data::item_id_remap::remap_item_id_for_version;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::clientbound::play::RECIPE_BOOK_ADD;
use pumpkin_data::recipes::{
    CookingRecipeType, CraftingRecipeTypes, RECIPES_COOKING, RECIPES_CRAFTING,
    RECIPES_STONECUTTING, RecipeCategoryTypes, RecipeIngredientTypes, RecipeResultStruct,
    StonecutterRecipe,
};
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;
use std::borrow::Cow;
use std::{collections::HashMap, io::Write};

use crate::codec::item_stack_seralizer::ItemStackTemplateSerializer;
use crate::{ClientPacket, VarInt, WritingError, ser::NetworkWriteExt};

use pumpkin_data::slot_display_id_remap::remap_slot_display_id_for_version;

// Recipe Display type IDs
const RECIPE_DISPLAY_SHAPELESS: i32 = 0;
const RECIPE_DISPLAY_SHAPED: i32 = 1;
const RECIPE_DISPLAY_FURNACE: i32 = 2;
// `RecipeDisplays.bootstrap` registers stonecutter fourth (`RecipeDisplays.java:5-11`).
const RECIPE_DISPLAY_STONECUTTER: i32 = 3;

// Slot Display base type IDs (26.2)
const SLOT_DISPLAY_EMPTY: u32 = 0;
const SLOT_DISPLAY_ANY_FUEL: u32 = 1;
const SLOT_DISPLAY_ITEM: u32 = 4;
const SLOT_DISPLAY_ITEM_STACK: u32 = 5;
const SLOT_DISPLAY_COMPOSITE: u32 = 10;

const ENTRY_FLAG_NOTIFICATION: u8 = 0x01;
const ENTRY_FLAG_HIGHLIGHT: u8 = 0x02;

// RecipeBookCategory IDs
const CATEGORY_CRAFTING_BUILDING: i32 = 0;
const CATEGORY_CRAFTING_REDSTONE: i32 = 1;
const CATEGORY_CRAFTING_EQUIPMENT: i32 = 2;
const CATEGORY_CRAFTING_MISC: i32 = 3;
const CATEGORY_FURNACE_FOOD: i32 = 4;
const CATEGORY_FURNACE_BLOCKS: i32 = 5;
const CATEGORY_FURNACE_MISC: i32 = 6;
const CATEGORY_BLAST_FURNACE_BLOCKS: i32 = 7;
const CATEGORY_BLAST_FURNACE_MISC: i32 = 8;
const CATEGORY_SMOKER_FOOD: i32 = 9;
// `RecipeBookCategories` registers stonecutter after smoker (`RecipeBookCategories.java:6-19`).
const CATEGORY_STONECUTTER: i32 = 10;
const CATEGORY_CAMPFIRE: i32 = 12;

use crate::codec::recipe::DynamicRecipe;

/// Clientbound packet that adds recipes to the client's recipe book.
/// `replace = true` means the client replaces its current recipe list.
#[java_packet(RECIPE_BOOK_ADD)]
pub struct CRecipeBookAdd<'a> {
    pub replace: bool,
    pub dynamic_recipes: &'a [DynamicRecipe],
}

impl<'a> CRecipeBookAdd<'a> {
    #[must_use]
    pub const fn new(replace: bool, dynamic_recipes: &'a [DynamicRecipe]) -> Self {
        Self {
            replace,
            dynamic_recipes,
        }
    }
}

fn item_id_versioned(item: &Item, version: JavaMinecraftVersion) -> i32 {
    remap_item_id_for_version(item.id, version) as i32
}

fn slot_display_item_type(version: JavaMinecraftVersion) -> i32 {
    remap_slot_display_id_for_version(SLOT_DISPLAY_ITEM, version) as i32
}

fn slot_display_composite_type(version: JavaMinecraftVersion) -> i32 {
    remap_slot_display_id_for_version(SLOT_DISPLAY_COMPOSITE, version) as i32
}

fn slot_display_item_stack_type(version: JavaMinecraftVersion) -> i32 {
    remap_slot_display_id_for_version(SLOT_DISPLAY_ITEM_STACK, version) as i32
}

fn write_item_slot_display(
    write: &mut impl Write,
    item: &Item,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(slot_display_item_type(version)))?;
    write.write_var_int(&VarInt(item_id_versioned(item, version)))?;
    Ok(())
}

fn write_item_stack_slot_display(
    write: &mut impl Write,
    item: &Item,
    count: u8,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(slot_display_item_stack_type(version)))?;
    let static_item = Item::from_id(item.id)
        .ok_or_else(|| WritingError::Message(format!("item id {} must exist", item.id)))?;
    ItemStackTemplateSerializer::from(ItemStack::new(count, static_item))
        .write_with_version(write, &version)
}

fn write_empty_slot_display(
    write: &mut impl Write,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(
        remap_slot_display_id_for_version(SLOT_DISPLAY_EMPTY, version) as i32,
    ))?;
    Ok(())
}

fn write_any_fuel_slot_display(
    write: &mut impl Write,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(
        remap_slot_display_id_for_version(SLOT_DISPLAY_ANY_FUEL, version) as i32,
    ))?;
    Ok(())
}

fn resolve_item_tag(tag: &str, version: JavaMinecraftVersion) -> Option<Vec<&'static Item>> {
    let tag = tag.strip_prefix('#').unwrap_or(tag);
    let full_tag = if tag.contains(':') {
        Cow::Borrowed(tag)
    } else {
        Cow::Owned(format!("minecraft:{tag}"))
    };

    let item_names =
        pumpkin_data::tag::get_registry_key_tags(version, pumpkin_data::tag::RegistryKey::Item)
            .and_then(|map| map.get(full_tag.as_ref()))
            .map(|t| t.0)
            .or_else(|| {
                pumpkin_data::tag::get_tag_values(
                    pumpkin_data::tag::RegistryKey::Item,
                    full_tag.as_ref(),
                )
            })?;

    let mut items = Vec::new();
    for name in item_names {
        let key = name.strip_prefix("minecraft:").unwrap_or(name);
        if let Some(item) = Item::from_registry_key(key) {
            items.push(item);
        }
    }
    if items.is_empty() { None } else { Some(items) }
}

fn write_ingredient_slot_display(
    write: &mut impl Write,
    ingredient: &RecipeIngredientTypes,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        RecipeIngredientTypes::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            if let Some(item) = Item::from_registry_key(key) {
                write_item_slot_display(write, item, version)?;
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        RecipeIngredientTypes::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag, version) {
                if items.len() == 1 {
                    write_item_slot_display(write, items[0], version)?;
                } else {
                    write.write_var_int(&VarInt(slot_display_composite_type(version)))?;
                    write.write_var_int(&VarInt(items.len() as i32))?;
                    for item in &items {
                        write_item_slot_display(write, item, version)?;
                    }
                }
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        RecipeIngredientTypes::OneOf(ids) => {
            let mut items: Vec<&Item> = Vec::new();
            for id in *ids {
                let key = id.strip_prefix("minecraft:").unwrap_or(id);
                if let Some(item) = Item::from_registry_key(key) {
                    items.push(item);
                }
            }
            if items.is_empty() {
                write_empty_slot_display(write, version)?;
            } else if items.len() == 1 {
                write_item_slot_display(write, items[0], version)?;
            } else {
                write.write_var_int(&VarInt(slot_display_composite_type(version)))?;
                write.write_var_int(&VarInt(items.len() as i32))?;
                for item in &items {
                    write_item_slot_display(write, item, version)?;
                }
            }
        }
    }
    Ok(())
}

/// Write a single Ingredient as a `HolderSet`<Item> for craftingRequirements.
///
/// Vanilla wire format for `ByteBufCodecs.holderSet(Registries.ITEM)`:
///   VarInt(0)     -> named tag reference (followed by `ResourceLocation`)
///   VarInt(n + 1) -> direct list of n item IDs
fn write_ingredient_holderset(
    write: &mut impl Write,
    ingredient: &RecipeIngredientTypes,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        RecipeIngredientTypes::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            // 1 item -> VarInt(1 + 1) = VarInt(2)
            write.write_var_int(&VarInt(2))?;
            if let Some(item) = Item::from_registry_key(key) {
                write.write_var_int(&VarInt(item_id_versioned(item, version)))?;
            } else {
                // Non-empty fallback item to prevent client UnsupportedOperationException
                write.write_var_int(&VarInt(0))?;
            }
        }
        RecipeIngredientTypes::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag, version) {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for item in &items {
                    write.write_var_int(&VarInt(item_id_versioned(item, version)))?;
                }
            } else {
                let tag = tag.strip_prefix('#').unwrap_or(tag);
                let full_tag = if tag.contains(':') {
                    tag.to_string()
                } else {
                    format!("minecraft:{tag}")
                };
                write.write_var_int(&VarInt(0))?;
                write.write_string(&full_tag)?;
            }
        }
        RecipeIngredientTypes::OneOf(ids) => {
            let items: Vec<i32> = ids
                .iter()
                .filter_map(|id| {
                    let key = id.strip_prefix("minecraft:").unwrap_or(id);
                    Item::from_registry_key(key).map(|item| item_id_versioned(item, version))
                })
                .collect();
            if items.is_empty() {
                write.write_var_int(&VarInt(2))?;
                write.write_var_int(&VarInt(0))?;
            } else {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for id in &items {
                    write.write_var_int(&VarInt(*id))?;
                }
            }
        }
    }
    Ok(())
}

/// Write the `craftingRequirements: Option<List<Ingredient>>` field (present).
fn write_crafting_requirements(
    write: &mut impl Write,
    slots: &[&RecipeIngredientTypes],
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_bool(true)?; // present
    write.write_var_int(&VarInt(slots.len() as i32))?;
    for slot in slots {
        write_ingredient_holderset(write, slot, version)?;
    }
    Ok(())
}

fn write_result_slot_display(
    write: &mut impl Write,
    result: &RecipeResultStruct,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let key = result.id.strip_prefix("minecraft:").unwrap_or(result.id);
    if let Some(item) = Item::from_registry_key(key) {
        write_item_stack_slot_display(write, item, result.count, version)?;
    } else {
        write_empty_slot_display(write, version)?;
    }
    Ok(())
}

fn write_optional_var_int(write: &mut impl Write, value: Option<i32>) -> Result<(), WritingError> {
    let encoded = value.map_or(Ok(0), |v| {
        v.checked_add(1)
            .ok_or_else(|| WritingError::Message(format!("group id {v} overflow")))
    })?;
    write.write_var_int(&VarInt(encoded))?;
    Ok(())
}
const fn entry_flags(replace: bool, notification: bool, highlight: bool) -> u8 {
    if replace {
        return 0;
    }

    (if notification {
        ENTRY_FLAG_NOTIFICATION
    } else {
        0
    }) | (if highlight { ENTRY_FLAG_HIGHLIGHT } else { 0 })
}

const fn crafting_category(cat: &RecipeCategoryTypes) -> i32 {
    match cat {
        RecipeCategoryTypes::Equipment => CATEGORY_CRAFTING_EQUIPMENT,
        RecipeCategoryTypes::Building | RecipeCategoryTypes::Blocks => CATEGORY_CRAFTING_BUILDING,
        RecipeCategoryTypes::Restone => CATEGORY_CRAFTING_REDSTONE,
        RecipeCategoryTypes::Food | RecipeCategoryTypes::Misc => CATEGORY_CRAFTING_MISC,
    }
}

/// Writes just the `RecipeDisplay`: the type tag, ingredients/pattern, result, and
/// crafting station.
///
/// This is the part shared between a full `RecipeDisplayEntry` (used by
/// `CRecipeBookAdd`) and a bare `ClientboundPlaceGhostRecipePacket`, which carries a
/// `RecipeDisplay` with no group/category/craftingRequirements/flags attached.
///
/// Returns `Ok(false)` without writing anything when the recipe has no displayable
/// form (special / decorated pot recipes).
#[allow(clippy::too_many_arguments)]
pub fn write_recipe_display(
    write: &mut impl Write,
    version: JavaMinecraftVersion,
    crafting_table: &Item,
    furnace: &Item,
    blast_furnace: &Item,
    smoker: &Item,
    campfire: &Item,
    crafting_recipe: Option<&CraftingRecipeTypes>,
    cooking_recipe: Option<&CookingRecipeType>,
) -> Result<bool, WritingError> {
    if let Some(recipe) = crafting_recipe {
        match recipe {
            CraftingRecipeTypes::CraftingShaped {
                pattern,
                key,
                result,
                ..
            } => {
                // Compute width and height from pattern
                let height = pattern.len() as i32;
                let width = pattern.first().map_or(0, |r| r.len()) as i32;

                // RecipeDisplay type = shaped (1)
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPED))?;
                // width, height
                write.write_var_int(&VarInt(width))?;
                write.write_var_int(&VarInt(height))?;
                // ingredients: flat list, row by row
                write.write_var_int(&VarInt(width * height))?;
                for row in *pattern {
                    for ch in row.chars() {
                        if ch == ' ' {
                            write_empty_slot_display(write, version)?;
                        } else if let Some((_, ingredient)) = key.iter().find(|(k, _)| *k == ch) {
                            write_ingredient_slot_display(write, ingredient, version)?;
                        } else {
                            write_empty_slot_display(write, version)?;
                        }
                    }
                }
                // result
                write_result_slot_display(write, result, version)?;
                // craftingStation
                write_item_slot_display(write, crafting_table, version)?;
            }
            CraftingRecipeTypes::CraftingShapeless {
                ingredients,
                result,
                ..
            } => {
                // RecipeDisplay type = shapeless (0)
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
                // ingredients list
                write.write_var_int(&VarInt(ingredients.len() as i32))?;
                for ing in *ingredients {
                    write_ingredient_slot_display(write, ing, version)?;
                }
                // result
                write_result_slot_display(write, result, version)?;
                // craftingStation
                write_item_slot_display(write, crafting_table, version)?;
            }
            CraftingRecipeTypes::CraftingTransmute {
                input,
                material,
                result,
                ..
            } => {
                // Transmute shown as shapeless with 2 ingredients
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
                write.write_var_int(&VarInt(2))?;
                write_ingredient_slot_display(write, input, version)?;
                write_ingredient_slot_display(write, material, version)?;
                write_result_slot_display(write, result, version)?;
                write_item_slot_display(write, crafting_table, version)?;
            }
            CraftingRecipeTypes::CraftingDye {
                target,
                dye,
                result,
                ..
            }
            | CraftingRecipeTypes::CraftingImbue {
                source: target,
                material: dye,
                result,
                ..
            } => {
                write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
                write.write_var_int(&VarInt(2))?;
                write_ingredient_slot_display(write, target, version)?;
                write_ingredient_slot_display(write, dye, version)?;
                write_result_slot_display(write, result, version)?;
                write_item_slot_display(write, crafting_table, version)?;
            }
            // Skip special/decorated_pot recipes as they have no useful display
            CraftingRecipeTypes::CraftingDecoratedPot { .. }
            | CraftingRecipeTypes::CraftingSpecial => {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    if let Some(recipe) = cooking_recipe {
        let (cooking, station) = match recipe {
            CookingRecipeType::Smelting(r) => (r, furnace),
            CookingRecipeType::Blasting(r) => (r, blast_furnace),
            CookingRecipeType::Smoking(r) => (r, smoker),
            CookingRecipeType::CampfireCooking(r) => (r, campfire),
        };

        // RecipeDisplay type = furnace (2)
        write.write_var_int(&VarInt(RECIPE_DISPLAY_FURNACE))?;
        // ingredient
        write_ingredient_slot_display(write, &cooking.ingredient, version)?;
        // fuel: AnyFuel
        write_any_fuel_slot_display(write, version)?;
        // result
        write_result_slot_display(write, &cooking.result, version)?;
        // craftingStation
        write_item_slot_display(write, station, version)?;
        // duration
        write.write_var_int(&VarInt(cooking.cookingtime))?;
        // experience
        write.write_f32_be(cooking.experience)?;
        return Ok(true);
    }

    Ok(false)
}

/// Writes vanilla `StonecutterRecipe.display()` as a
/// `StonecutterRecipeDisplay` (`StonecutterRecipe.java:37-44`).
fn write_stonecutter_recipe_display(
    write: &mut impl Write,
    version: JavaMinecraftVersion,
    stonecutter: &Item,
    recipe: &StonecutterRecipe,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(RECIPE_DISPLAY_STONECUTTER))?;
    write_ingredient_slot_display(write, &recipe.ingredient, version)?;
    write_result_slot_display(write, &recipe.result, version)?;
    write_item_slot_display(write, stonecutter, version)?;
    Ok(())
}

/// Write a single `RecipeDisplayEntry` + flags byte.
/// Returns `Ok(true)` if written, `Ok(false)` if skipped (special recipe).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn write_entry(
    write: &mut impl Write,
    display_id: i32,
    version: JavaMinecraftVersion,
    group_id: Option<i32>,
    flags: u8,
    crafting_table: &Item,
    furnace: &Item,
    blast_furnace: &Item,
    smoker: &Item,
    stonecutter: &Item,
    campfire: &Item,
    crafting_recipe: Option<&CraftingRecipeTypes>,
    cooking_recipe: Option<(&CookingRecipeType, i32)>,
    stonecutter_recipe: Option<&StonecutterRecipe>,
) -> Result<bool, WritingError> {
    if let Some(recipe) = crafting_recipe {
        // Bail out before a single byte is written: these have no RecipeDisplay, and
        // emitting the display id for them would leave a stray VarInt in the stream.
        if matches!(
            recipe,
            CraftingRecipeTypes::CraftingDecoratedPot { .. } | CraftingRecipeTypes::CraftingSpecial
        ) {
            return Ok(false);
        }

        // RecipeDisplayId
        write.write_var_int(&VarInt(display_id))?;
        write_recipe_display(
            write,
            version,
            crafting_table,
            furnace,
            blast_furnace,
            smoker,
            campfire,
            Some(recipe),
            None,
        )?;

        match recipe {
            CraftingRecipeTypes::CraftingShaped {
                category,
                pattern,
                key,
                ..
            } => {
                // group: OptionalVarInt
                write_optional_var_int(write, group_id)?;
                // category
                write.write_var_int(&VarInt(crafting_category(category)))?;
                // craftingRequirements: one HolderSet per non-empty grid slot
                // (Ingredient cannot be empty, so empty slots must be excluded)
                let mut slots: Vec<&RecipeIngredientTypes> = Vec::new();
                for row in *pattern {
                    for ch in row.chars() {
                        if ch != ' '
                            && let Some((_, ing)) = key.iter().find(|(k, _)| *k == ch)
                        {
                            slots.push(ing);
                        }
                    }
                }
                write_crafting_requirements(write, &slots, version)?;
            }
            CraftingRecipeTypes::CraftingShapeless {
                category,
                ingredients,
                ..
            } => {
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                // craftingRequirements: one HolderSet per ingredient
                let slots: Vec<&RecipeIngredientTypes> = ingredients.iter().collect();
                write_crafting_requirements(write, &slots, version)?;
            }
            CraftingRecipeTypes::CraftingTransmute {
                category,
                input,
                material,
                ..
            } => {
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                // craftingRequirements: input + material
                write_crafting_requirements(write, &[input, material], version)?;
            }
            CraftingRecipeTypes::CraftingDye {
                category,
                target,
                dye,
                ..
            } => {
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                write_crafting_requirements(write, &[target, dye], version)?;
            }
            CraftingRecipeTypes::CraftingImbue {
                category,
                source,
                material,
                ..
            } => {
                write_optional_var_int(write, group_id)?;
                write.write_var_int(&VarInt(crafting_category(category)))?;
                write_crafting_requirements(write, &[source, material], version)?;
            }
            // Excluded by the guard above.
            CraftingRecipeTypes::CraftingDecoratedPot { .. }
            | CraftingRecipeTypes::CraftingSpecial => return Ok(false),
        }
        write.write_u8(flags)?;
        return Ok(true);
    }

    if let Some((recipe, book_category)) = cooking_recipe {
        write.write_var_int(&VarInt(display_id))?;
        write_recipe_display(
            write,
            version,
            crafting_table,
            furnace,
            blast_furnace,
            smoker,
            campfire,
            None,
            Some(recipe),
        )?;

        let cooking = match recipe {
            CookingRecipeType::Smelting(r)
            | CookingRecipeType::Blasting(r)
            | CookingRecipeType::Smoking(r)
            | CookingRecipeType::CampfireCooking(r) => r,
        };
        // group: OptionalVarInt
        write_optional_var_int(write, group_id)?;
        // category
        write.write_var_int(&VarInt(book_category))?;
        // craftingRequirements: the single ingredient
        write_crafting_requirements(write, &[&cooking.ingredient], version)?;
        write.write_u8(flags)?;
        return Ok(true);
    }

    if let Some(recipe) = stonecutter_recipe {
        write.write_var_int(&VarInt(display_id))?;
        write_stonecutter_recipe_display(write, version, stonecutter, recipe)?;
        // `StonecutterRecipe.recipeBookCategory()` is the registered stonecutter category
        // (`StonecutterRecipe.java:46-49`), and its single input is the crafting requirement.
        write_optional_var_int(write, group_id)?;
        write.write_var_int(&VarInt(CATEGORY_STONECUTTER))?;
        write_crafting_requirements(write, &[&recipe.ingredient], version)?;
        write.write_u8(flags)?;
        return Ok(true);
    }

    Ok(false)
}

#[allow(clippy::too_many_lines)]
impl ClientPacket for CRecipeBookAdd<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let mut write = write;

        // Station items (these IDs are stable across all versions we support)
        let crafting_table = Item::from_registry_key("crafting_table")
            .ok_or_else(|| WritingError::Message("crafting_table item must exist".into()))?;
        let furnace = Item::from_registry_key("furnace")
            .ok_or_else(|| WritingError::Message("furnace item must exist".into()))?;
        let blast_furnace = Item::from_registry_key("blast_furnace")
            .ok_or_else(|| WritingError::Message("blast_furnace item must exist".into()))?;
        let smoker = Item::from_registry_key("smoker")
            .ok_or_else(|| WritingError::Message("smoker item must exist".into()))?;
        let stonecutter = Item::from_registry_key("stonecutter")
            .ok_or_else(|| WritingError::Message("stonecutter item must exist".into()))?;
        let campfire = Item::from_registry_key("campfire")
            .ok_or_else(|| WritingError::Message("campfire item must exist".into()))?;

        // First pass - count and skip CraftingSpecial and CraftingDecoratedPot entries.
        let crafting_count: usize = RECIPES_CRAFTING
            .iter()
            .filter(|r| {
                !matches!(
                    r,
                    CraftingRecipeTypes::CraftingSpecial
                        | CraftingRecipeTypes::CraftingDecoratedPot { .. }
                )
            })
            .count();
        let dynamic_count = self.dynamic_recipes.len();
        // `StonecutterRecipe.display()` contributes one entry per static recipe
        // (`StonecutterRecipe.java:37-49`).
        let total =
            crafting_count + RECIPES_COOKING.len() + RECIPES_STONECUTTING.len() + dynamic_count;

        // Entry count (VarInt)
        write.write_var_int(&VarInt(total as i32))?;

        let mut display_id: i32 = 0;
        let mut group_ids: HashMap<Cow<'_, str>, i32> = HashMap::new();
        let mut next_group_id: i32 = 0;
        let highlight = !self.replace;

        // Write crafting recipes
        for recipe in RECIPES_CRAFTING {
            let (group, notification) = match recipe {
                CraftingRecipeTypes::CraftingShaped {
                    group,
                    show_notification,
                    ..
                } => (group.map(Cow::Borrowed), *show_notification),
                CraftingRecipeTypes::CraftingShapeless { group, .. }
                | CraftingRecipeTypes::CraftingTransmute { group, .. }
                | CraftingRecipeTypes::CraftingDye { group, .. }
                | CraftingRecipeTypes::CraftingImbue { group, .. } => {
                    (group.map(Cow::Borrowed), true)
                }
                CraftingRecipeTypes::CraftingDecoratedPot { .. }
                | CraftingRecipeTypes::CraftingSpecial => (None, true),
            };
            let group_id = resolve_group_id_owned(&mut group_ids, &mut next_group_id, group);
            let flags = entry_flags(self.replace, notification, highlight);
            let written = write_entry(
                &mut write,
                display_id,
                *version,
                group_id,
                flags,
                crafting_table,
                furnace,
                blast_furnace,
                smoker,
                stonecutter,
                campfire,
                Some(recipe),
                None,
                None,
            )?;
            if written {
                display_id += 1;
            }
        }

        // Write cooking recipes
        for recipe in RECIPES_COOKING {
            let (book_category, group) = match recipe {
                CookingRecipeType::Smelting(r) => (
                    match r.category {
                        RecipeCategoryTypes::Food => CATEGORY_FURNACE_FOOD,
                        RecipeCategoryTypes::Blocks => CATEGORY_FURNACE_BLOCKS,
                        _ => CATEGORY_FURNACE_MISC,
                    },
                    r.group,
                ),
                CookingRecipeType::Blasting(r) => (
                    match r.category {
                        RecipeCategoryTypes::Blocks => CATEGORY_BLAST_FURNACE_BLOCKS,
                        _ => CATEGORY_BLAST_FURNACE_MISC,
                    },
                    r.group,
                ),
                CookingRecipeType::Smoking(r) => (CATEGORY_SMOKER_FOOD, r.group),
                CookingRecipeType::CampfireCooking(r) => (CATEGORY_CAMPFIRE, r.group),
            };
            let group_id = resolve_group_id_owned(
                &mut group_ids,
                &mut next_group_id,
                group.map(Cow::Borrowed),
            );
            let flags = entry_flags(self.replace, true, highlight);
            write_entry(
                &mut write,
                display_id,
                *version,
                group_id,
                flags,
                crafting_table,
                furnace,
                blast_furnace,
                smoker,
                stonecutter,
                campfire,
                None,
                Some((recipe, book_category)),
                None,
            )?;
            display_id += 1;
        }

        // Vanilla `StonecutterRecipe.display()` contributes a static recipe-book entry
        // (`StonecutterRecipe.java:37-49`) after the cooking displays.
        for recipe in RECIPES_STONECUTTING {
            let flags = entry_flags(self.replace, true, highlight);
            write_entry(
                &mut write,
                display_id,
                *version,
                None,
                flags,
                crafting_table,
                furnace,
                blast_furnace,
                smoker,
                stonecutter,
                campfire,
                None,
                None,
                Some(recipe),
            )?;
            display_id += 1;
        }

        // Write dynamic recipes
        for recipe in self.dynamic_recipes {
            match recipe {
                DynamicRecipe::Crafting(crafting) => {
                    let (group, flags) = match crafting {
                        crate::codec::recipe::OwnedCraftingRecipe::Shaped {
                            group,
                            show_notification,
                            ..
                        } => (
                            group.as_deref().map(Cow::Borrowed),
                            entry_flags(self.replace, *show_notification, highlight),
                        ),
                        crate::codec::recipe::OwnedCraftingRecipe::Shapeless { group, .. }
                        | crate::codec::recipe::OwnedCraftingRecipe::Dye { group, .. }
                        | crate::codec::recipe::OwnedCraftingRecipe::Imbue { group, .. } => (
                            group.as_deref().map(Cow::Borrowed),
                            entry_flags(self.replace, true, highlight),
                        ),
                    };
                    let group_id =
                        resolve_group_id_owned(&mut group_ids, &mut next_group_id, group);
                    write_dynamic_crafting_entry(
                        &mut write,
                        display_id,
                        *version,
                        group_id,
                        flags,
                        crafting_table,
                        crafting,
                    )?;
                }
                DynamicRecipe::Cooking(cooking) => {
                    let (book_category, group, owned_cooking) = match cooking {
                        crate::codec::recipe::OwnedCookingRecipeType::Smelting(r) => (
                            match r.category {
                                RecipeCategoryTypes::Food => CATEGORY_FURNACE_FOOD,
                                RecipeCategoryTypes::Blocks => CATEGORY_FURNACE_BLOCKS,
                                _ => CATEGORY_FURNACE_MISC,
                            },
                            r.group.as_deref().map(Cow::Borrowed),
                            r,
                        ),
                        crate::codec::recipe::OwnedCookingRecipeType::Blasting(r) => (
                            match r.category {
                                RecipeCategoryTypes::Blocks => CATEGORY_BLAST_FURNACE_BLOCKS,
                                _ => CATEGORY_BLAST_FURNACE_MISC,
                            },
                            r.group.as_deref().map(Cow::Borrowed),
                            r,
                        ),
                        crate::codec::recipe::OwnedCookingRecipeType::Smoking(r) => (
                            CATEGORY_SMOKER_FOOD,
                            r.group.as_deref().map(Cow::Borrowed),
                            r,
                        ),
                        crate::codec::recipe::OwnedCookingRecipeType::CampfireCooking(r) => {
                            (CATEGORY_CAMPFIRE, r.group.as_deref().map(Cow::Borrowed), r)
                        }
                    };
                    let station = match cooking {
                        crate::codec::recipe::OwnedCookingRecipeType::Smelting(_) => furnace,
                        crate::codec::recipe::OwnedCookingRecipeType::Blasting(_) => blast_furnace,
                        crate::codec::recipe::OwnedCookingRecipeType::Smoking(_) => smoker,
                        crate::codec::recipe::OwnedCookingRecipeType::CampfireCooking(_) => {
                            campfire
                        }
                    };

                    let group_id =
                        resolve_group_id_owned(&mut group_ids, &mut next_group_id, group);
                    let flags = entry_flags(self.replace, true, highlight);
                    write_dynamic_cooking_entry(
                        &mut write,
                        display_id,
                        *version,
                        group_id,
                        flags,
                        station,
                        owned_cooking,
                        book_category,
                    )?;
                }
            }
            display_id += 1;
        }

        // replace flag
        write.write_bool(self.replace)?;
        Ok(())
    }
}

fn resolve_group_id_owned<'a>(
    group_ids: &mut HashMap<Cow<'a, str>, i32>,
    next_group_id: &mut i32,
    group: Option<Cow<'a, str>>,
) -> Option<i32> {
    let key = group?;
    Some(*group_ids.entry(key).or_insert_with(|| {
        let id = *next_group_id;
        *next_group_id += 1;
        id
    }))
}

fn write_dynamic_ingredient_slot_display(
    write: &mut impl Write,
    ingredient: &crate::codec::recipe::OwnedRecipeIngredient,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        crate::codec::recipe::OwnedRecipeIngredient::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            if let Some(item) = Item::from_registry_key(key) {
                write_item_slot_display(write, item, version)?;
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag, version) {
                if items.len() == 1 {
                    write_item_slot_display(write, items[0], version)?;
                } else {
                    write.write_var_int(&VarInt(slot_display_composite_type(version)))?;
                    write.write_var_int(&VarInt(items.len() as i32))?;
                    for item in &items {
                        write_item_slot_display(write, item, version)?;
                    }
                }
            } else {
                write_empty_slot_display(write, version)?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::OneOf(ids) => {
            let items: Vec<&Item> = ids
                .iter()
                .filter_map(|id| {
                    let key = id.strip_prefix("minecraft:").unwrap_or(id);
                    Item::from_registry_key(key)
                })
                .collect();

            if items.is_empty() {
                write_empty_slot_display(write, version)?;
            } else if items.len() == 1 {
                write_item_slot_display(write, items[0], version)?;
            } else {
                write.write_var_int(&VarInt(slot_display_composite_type(version)))?;
                write.write_var_int(&VarInt(items.len() as i32))?;
                for item in &items {
                    write_item_slot_display(write, item, version)?;
                }
            }
        }
    }
    Ok(())
}

fn write_dynamic_ingredient_holderset(
    write: &mut impl Write,
    ingredient: &crate::codec::recipe::OwnedRecipeIngredient,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    match ingredient {
        crate::codec::recipe::OwnedRecipeIngredient::Simple(id) => {
            let key = id.strip_prefix("minecraft:").unwrap_or(id);
            write.write_var_int(&VarInt(2))?;
            if let Some(item) = Item::from_registry_key(key) {
                write.write_var_int(&VarInt(item_id_versioned(item, version)))?;
            } else {
                write.write_var_int(&VarInt(0))?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::Tagged(tag) => {
            if let Some(items) = resolve_item_tag(tag, version) {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for item in &items {
                    write.write_var_int(&VarInt(item_id_versioned(item, version)))?;
                }
            } else {
                let tag = tag.strip_prefix('#').unwrap_or(tag);
                let full_tag = if tag.contains(':') {
                    tag.to_string()
                } else {
                    format!("minecraft:{tag}")
                };
                write.write_var_int(&VarInt(0))?;
                write.write_string(&full_tag)?;
            }
        }
        crate::codec::recipe::OwnedRecipeIngredient::OneOf(ids) => {
            let items: Vec<i32> = ids
                .iter()
                .filter_map(|id| {
                    let key = id.strip_prefix("minecraft:").unwrap_or(id);
                    Item::from_registry_key(key).map(|item| item_id_versioned(item, version))
                })
                .collect();
            if items.is_empty() {
                write.write_var_int(&VarInt(2))?;
                write.write_var_int(&VarInt(0))?;
            } else {
                write.write_var_int(&VarInt(items.len() as i32 + 1))?;
                for id in &items {
                    write.write_var_int(&VarInt(*id))?;
                }
            }
        }
    }
    Ok(())
}

fn write_dynamic_result_slot_display(
    write: &mut impl Write,
    result: &crate::codec::recipe::OwnedRecipeResult,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let key = result
        .item_id
        .strip_prefix("minecraft:")
        .unwrap_or(&result.item_id);
    if let Some(item) = Item::from_registry_key(key) {
        write_item_stack_slot_display(write, item, result.count, version)?;
    } else {
        write_empty_slot_display(write, version)?;
    }
    Ok(())
}

/// Writes just the `RecipeDisplay` for a dynamic (data-pack) crafting recipe --
/// the counterpart to [`write_recipe_display`] for `OwnedCraftingRecipe`.
pub fn write_dynamic_recipe_display(
    write: &mut impl Write,
    version: JavaMinecraftVersion,
    crafting_table: &Item,
    recipe: &crate::codec::recipe::OwnedCraftingRecipe,
) -> Result<(), WritingError> {
    match recipe {
        crate::codec::recipe::OwnedCraftingRecipe::Shaped {
            pattern,
            key,
            result,
            ..
        } => {
            let height = pattern.len() as i32;
            let width = pattern.first().map_or(0, String::len) as i32;

            write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPED))?;
            write.write_var_int(&VarInt(width))?;
            write.write_var_int(&VarInt(height))?;
            write.write_var_int(&VarInt(width * height))?;
            for row in pattern {
                for ch in row.chars() {
                    if ch == ' ' {
                        write_empty_slot_display(write, version)?;
                    } else if let Some((_, ingredient)) = key.iter().find(|(k, _)| *k == ch) {
                        write_dynamic_ingredient_slot_display(write, ingredient, version)?;
                    } else {
                        write_empty_slot_display(write, version)?;
                    }
                }
            }
            write_dynamic_result_slot_display(write, result, version)?;
            write_item_slot_display(write, crafting_table, version)?;
        }
        crate::codec::recipe::OwnedCraftingRecipe::Shapeless {
            ingredients,
            result,
            ..
        } => {
            write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
            write.write_var_int(&VarInt(ingredients.len() as i32))?;
            for ing in ingredients {
                write_dynamic_ingredient_slot_display(write, ing, version)?;
            }
            write_dynamic_result_slot_display(write, result, version)?;
            write_item_slot_display(write, crafting_table, version)?;
        }
        crate::codec::recipe::OwnedCraftingRecipe::Dye {
            target,
            dye,
            result,
            ..
        }
        | crate::codec::recipe::OwnedCraftingRecipe::Imbue {
            source: target,
            material: dye,
            result,
            ..
        } => {
            write.write_var_int(&VarInt(RECIPE_DISPLAY_SHAPELESS))?;
            write.write_var_int(&VarInt(2))?;
            write_dynamic_ingredient_slot_display(write, target, version)?;
            write_dynamic_ingredient_slot_display(write, dye, version)?;
            write_dynamic_result_slot_display(write, result, version)?;
            write_item_slot_display(write, crafting_table, version)?;
        }
    }
    Ok(())
}

fn write_dynamic_crafting_entry(
    write: &mut impl Write,
    display_id: i32,
    version: JavaMinecraftVersion,
    group_id: Option<i32>,
    flags: u8,
    crafting_table: &Item,
    recipe: &crate::codec::recipe::OwnedCraftingRecipe,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(display_id))?;
    write_dynamic_recipe_display(write, version, crafting_table, recipe)?;

    match recipe {
        crate::codec::recipe::OwnedCraftingRecipe::Shaped {
            category,
            pattern,
            key,
            ..
        } => {
            write_optional_var_int(write, group_id)?;
            write.write_var_int(&VarInt(crafting_category(category)))?;

            let mut slots: Vec<&crate::codec::recipe::OwnedRecipeIngredient> = Vec::new();
            for row in pattern {
                for ch in row.chars() {
                    if ch != ' '
                        && let Some((_, ing)) = key.iter().find(|(k, _)| *k == ch)
                    {
                        slots.push(ing);
                    }
                }
            }
            write.write_bool(true)?; // present
            write.write_var_int(&VarInt(slots.len() as i32))?;
            for ing in slots {
                write_dynamic_ingredient_holderset(write, ing, version)?;
            }
        }
        crate::codec::recipe::OwnedCraftingRecipe::Shapeless {
            category,
            ingredients,
            ..
        } => {
            write_optional_var_int(write, group_id)?;
            write.write_var_int(&VarInt(crafting_category(category)))?;

            write.write_bool(true)?;
            write.write_var_int(&VarInt(ingredients.len() as i32))?;
            for ing in ingredients {
                write_dynamic_ingredient_holderset(write, ing, version)?;
            }
        }
        crate::codec::recipe::OwnedCraftingRecipe::Dye {
            category,
            target,
            dye,
            ..
        }
        | crate::codec::recipe::OwnedCraftingRecipe::Imbue {
            category,
            source: target,
            material: dye,
            ..
        } => {
            write_optional_var_int(write, group_id)?;
            write.write_var_int(&VarInt(crafting_category(category)))?;
            write.write_bool(true)?;
            write.write_var_int(&VarInt(2))?;
            write_dynamic_ingredient_holderset(write, target, version)?;
            write_dynamic_ingredient_holderset(write, dye, version)?;
        }
    }
    write.write_u8(flags)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_dynamic_cooking_entry(
    write: &mut impl Write,
    display_id: i32,
    version: JavaMinecraftVersion,
    group_id: Option<i32>,
    flags: u8,
    station: &Item,
    cooking: &crate::codec::recipe::OwnedCookingRecipe,
    book_category: i32,
) -> Result<(), WritingError> {
    write.write_var_int(&VarInt(display_id))?;
    write.write_var_int(&VarInt(RECIPE_DISPLAY_FURNACE))?;
    write_dynamic_ingredient_slot_display(write, &cooking.ingredient, version)?;
    write_any_fuel_slot_display(write, version)?;
    write_dynamic_result_slot_display(write, &cooking.result, version)?;
    write_item_slot_display(write, station, version)?;
    write.write_var_int(&VarInt(cooking.cooking_time))?;
    write.write_f32_be(cooking.experience)?;
    write_optional_var_int(write, group_id)?;
    write.write_var_int(&VarInt(book_category))?;
    write.write_bool(true)?;
    write.write_var_int(&VarInt(1))?;
    write_dynamic_ingredient_holderset(write, &cooking.ingredient, version)?;
    write.write_u8(flags)?;
    Ok(())
}

/// One `ClientboundRecipeBookAddPacket.Entry`
/// (`ClientboundRecipeBookAddPacket.java:29-50`) named by the display id the static
/// recipe tables assign it, plus the two flag bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecipeBookEntry {
    /// `RecipeDisplayEntry.id` (`RecipeDisplayId.java:7`).
    pub display_id: i32,
    /// `FLAG_NOTIFICATION` (`ClientboundRecipeBookAddPacket.java:30`).
    pub notification: bool,
    /// `FLAG_HIGHLIGHT` (`ClientboundRecipeBookAddPacket.java:31`).
    pub highlight: bool,
}

impl RecipeBookEntry {
    /// A newly unlocked recipe, `ServerRecipeBook.addRecipes`
    /// (`ServerRecipeBook.java:70-72`): notification from the recipe's own
    /// `showNotification`, highlight always set.
    #[must_use]
    pub fn unlocked(display_id: i32) -> Self {
        Self {
            display_id,
            notification: show_notification(display_id),
            highlight: true,
        }
    }

    /// The `flags` byte, `ClientboundRecipeBookAddPacket.Entry`
    /// (`ClientboundRecipeBookAddPacket.java:40-42`): bit 0 notification, bit 1
    /// highlight. Unlike [`entry_flags`], vanilla never clears these for a
    /// `replace` packet.
    #[must_use]
    pub const fn flags(&self) -> u8 {
        (if self.notification {
            ENTRY_FLAG_NOTIFICATION
        } else {
            0
        }) | (if self.highlight {
            ENTRY_FLAG_HIGHLIGHT
        } else {
            0
        })
    }

    /// An already-known recipe, `ServerRecipeBook.sendInitialRecipeBook`
    /// (`ServerRecipeBook.java:117`): notification never set, highlight only for a
    /// recipe the player has not seen displayed yet.
    #[must_use]
    pub const fn known(display_id: i32, highlight: bool) -> Self {
        Self {
            display_id,
            notification: false,
            highlight,
        }
    }
}

/// `Recipe.showNotification()` for the recipe that carries this display id.
/// Only shaped crafting recipes model the flag; everything else defaults to true,
/// exactly as [`CRecipeBookAdd`] assumes.
#[must_use]
fn show_notification(display_id: i32) -> bool {
    let mut index = 0;
    for recipe in RECIPES_CRAFTING {
        let notification = match recipe {
            CraftingRecipeTypes::CraftingShaped {
                show_notification, ..
            } => *show_notification,
            CraftingRecipeTypes::CraftingShapeless { .. }
            | CraftingRecipeTypes::CraftingTransmute { .. }
            | CraftingRecipeTypes::CraftingDye { .. }
            | CraftingRecipeTypes::CraftingImbue { .. } => true,
            CraftingRecipeTypes::CraftingDecoratedPot { .. }
            | CraftingRecipeTypes::CraftingSpecial => continue,
        };
        if index == display_id {
            return notification;
        }
        index += 1;
    }
    true
}

/// `ClientboundRecipeBookAddPacket` carrying a per-player subset.
///
/// A subset is what vanilla always sends: `ServerRecipeBook.addRecipes`
/// (`ServerRecipeBook.java:61-80`) sends only the newly unlocked entries with
/// `replace = false`, and `ServerRecipeBook.sendInitialRecipeBook`
/// (`ServerRecipeBook.java:112-121`) sends the known set with `replace = true`.
///
/// [`CRecipeBookAdd`] emits every entry of `RECIPES_CRAFTING`, `RECIPES_COOKING`, and
/// `RECIPES_STONECUTTING`
/// unconditionally and so cannot express either; this type shares its entry writer
/// and its display-id numbering, so an entry written here is byte-identical to the
/// same entry written there.
///
/// Note that vanilla's per-entry flags are independent of `replace`
/// (`ClientboundRecipeBookAddPacket.java:40-42`), unlike [`entry_flags`], which
/// zeroes them whenever `replace` is set.
#[java_packet(RECIPE_BOOK_ADD)]
pub struct CRecipeBookAddSubset<'a> {
    pub replace: bool,
    pub entries: &'a [RecipeBookEntry],
}

impl<'a> CRecipeBookAddSubset<'a> {
    #[must_use]
    pub const fn new(replace: bool, entries: &'a [RecipeBookEntry]) -> Self {
        Self { replace, entries }
    }
}

/// The six crafting-station items every static `RecipeDisplay` names.
struct Stations {
    crafting_table: &'static Item,
    furnace: &'static Item,
    blast_furnace: &'static Item,
    smoker: &'static Item,
    stonecutter: &'static Item,
    campfire: &'static Item,
}

impl Stations {
    fn resolve() -> Result<Self, WritingError> {
        let get = |key: &str| {
            Item::from_registry_key(key)
                .ok_or_else(|| WritingError::Message(format!("{key} item must exist")))
        };
        Ok(Self {
            crafting_table: get("crafting_table")?,
            furnace: get("furnace")?,
            blast_furnace: get("blast_furnace")?,
            smoker: get("smoker")?,
            stonecutter: get("stonecutter")?,
            campfire: get("campfire")?,
        })
    }
}

/// Shared state of one subset write: the display-id cursor and the group-id table,
/// both advanced for every eligible recipe so that a selected entry lands on exactly
/// the id and group [`CRecipeBookAdd`] would have given it.
struct SubsetWriter<'a> {
    wanted: HashMap<i32, RecipeBookEntry>,
    stations: Stations,
    version: JavaMinecraftVersion,
    body: Vec<u8>,
    written: i32,
    display_id: i32,
    group_ids: HashMap<Cow<'a, str>, i32>,
    next_group_id: i32,
}

impl SubsetWriter<'_> {
    fn write_crafting(&mut self) -> Result<(), WritingError> {
        for recipe in RECIPES_CRAFTING {
            let group = match recipe {
                CraftingRecipeTypes::CraftingShaped { group, .. }
                | CraftingRecipeTypes::CraftingShapeless { group, .. }
                | CraftingRecipeTypes::CraftingTransmute { group, .. }
                | CraftingRecipeTypes::CraftingDye { group, .. }
                | CraftingRecipeTypes::CraftingImbue { group, .. } => group.map(Cow::Borrowed),
                // No display, no display id: `CRecipeBookAdd` skips these too.
                CraftingRecipeTypes::CraftingDecoratedPot { .. }
                | CraftingRecipeTypes::CraftingSpecial => continue,
            };
            let group_id =
                resolve_group_id_owned(&mut self.group_ids, &mut self.next_group_id, group);
            if let Some(entry) = self.wanted.get(&self.display_id).copied() {
                self.emit(entry, group_id, Some(recipe), None, None)?;
            }
            self.display_id += 1;
        }
        Ok(())
    }

    fn write_cooking(&mut self) -> Result<(), WritingError> {
        for recipe in RECIPES_COOKING {
            let (book_category, group) = cooking_category_and_group(recipe);
            let group_id = resolve_group_id_owned(
                &mut self.group_ids,
                &mut self.next_group_id,
                group.map(Cow::Borrowed),
            );
            if let Some(entry) = self.wanted.get(&self.display_id).copied() {
                self.emit(entry, group_id, None, Some((recipe, book_category)), None)?;
            }
            self.display_id += 1;
        }
        Ok(())
    }

    fn write_stonecutter(&mut self) -> Result<(), WritingError> {
        for recipe in RECIPES_STONECUTTING {
            if let Some(entry) = self.wanted.get(&self.display_id).copied() {
                self.emit(entry, None, None, None, Some(recipe))?;
            }
            self.display_id += 1;
        }
        Ok(())
    }

    fn emit(
        &mut self,
        entry: RecipeBookEntry,
        group_id: Option<i32>,
        crafting_recipe: Option<&CraftingRecipeTypes>,
        cooking_recipe: Option<(&CookingRecipeType, i32)>,
        stonecutter_recipe: Option<&StonecutterRecipe>,
    ) -> Result<(), WritingError> {
        if write_entry(
            &mut self.body,
            self.display_id,
            self.version,
            group_id,
            entry.flags(),
            self.stations.crafting_table,
            self.stations.furnace,
            self.stations.blast_furnace,
            self.stations.smoker,
            self.stations.stonecutter,
            self.stations.campfire,
            crafting_recipe,
            cooking_recipe,
            stonecutter_recipe,
        )? {
            self.written += 1;
        }
        Ok(())
    }
}

/// The recipe-book category id and group of one cooking recipe, matching what
/// [`CRecipeBookAdd`] derives for the same entry.
const fn cooking_category_and_group(recipe: &CookingRecipeType) -> (i32, Option<&'static str>) {
    match recipe {
        CookingRecipeType::Smelting(r) => (
            match r.category {
                RecipeCategoryTypes::Food => CATEGORY_FURNACE_FOOD,
                RecipeCategoryTypes::Blocks => CATEGORY_FURNACE_BLOCKS,
                _ => CATEGORY_FURNACE_MISC,
            },
            r.group,
        ),
        CookingRecipeType::Blasting(r) => (
            match r.category {
                RecipeCategoryTypes::Blocks => CATEGORY_BLAST_FURNACE_BLOCKS,
                _ => CATEGORY_BLAST_FURNACE_MISC,
            },
            r.group,
        ),
        CookingRecipeType::Smoking(r) => (CATEGORY_SMOKER_FOOD, r.group),
        CookingRecipeType::CampfireCooking(r) => (CATEGORY_CAMPFIRE, r.group),
    }
}

impl ClientPacket for CRecipeBookAddSubset<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let mut write = write;
        let mut writer = SubsetWriter {
            wanted: self
                .entries
                .iter()
                .map(|entry| (entry.display_id, *entry))
                .collect(),
            stations: Stations::resolve()?,
            version: *version,
            // Entries go into a scratch buffer first so the leading count can never
            // disagree with the number of entries that actually got written.
            body: Vec::new(),
            written: 0,
            display_id: 0,
            group_ids: HashMap::new(),
            next_group_id: 0,
        };

        writer.write_crafting()?;
        writer.write_cooking()?;
        writer.write_stonecutter()?;

        write.write_var_int(&VarInt(writer.written))?;
        write.write_slice(&writer.body)?;
        write.write_bool(self.replace)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: JavaMinecraftVersion = JavaMinecraftVersion::V_26_2;

    fn encode_subset(replace: bool, entries: &[RecipeBookEntry]) -> Vec<u8> {
        let mut buf = Vec::new();
        CRecipeBookAddSubset::new(replace, entries)
            .write_packet_data(&mut buf, &VERSION)
            .expect("write");
        buf
    }

    #[test]
    fn empty_subset_is_count_zero_then_the_replace_flag() {
        assert_eq!(encode_subset(false, &[]), vec![0x00, 0x00]);
        assert_eq!(encode_subset(true, &[]), vec![0x00, 0x01]);
    }

    #[test]
    fn entry_count_leads_and_replace_flag_trails() {
        let bytes = encode_subset(
            true,
            &[RecipeBookEntry {
                display_id: 0,
                notification: false,
                highlight: false,
            }],
        );
        assert_eq!(bytes[0], 0x01, "one entry");
        // The entry's own first byte is its RecipeDisplayId VarInt.
        assert_eq!(bytes[1], 0x00);
        assert_eq!(*bytes.last().expect("non-empty"), 0x01, "replace = true");
    }

    #[test]
    fn flags_byte_is_the_last_byte_of_an_entry_and_ignores_replace() {
        for (notification, highlight, expected) in [
            (false, false, 0x00u8),
            (true, false, 0x01),
            (false, true, 0x02),
            (true, true, 0x03),
        ] {
            for replace in [false, true] {
                let bytes = encode_subset(
                    replace,
                    &[RecipeBookEntry {
                        display_id: 0,
                        notification,
                        highlight,
                    }],
                );
                let len = bytes.len();
                assert_eq!(
                    bytes[len - 2],
                    expected,
                    "flags for ({notification}, {highlight}, replace={replace})"
                );
            }
        }
    }

    /// An entry written here must be byte-identical to the same entry written by the
    /// full-table writer, so the client's `RecipeDisplayId`s keep meaning the same
    /// recipe whichever packet delivered them.
    #[test]
    fn subset_entries_match_the_full_packet_byte_for_byte() {
        let full = {
            let mut buf = Vec::new();
            CRecipeBookAdd::new(false, &[])
                .write_packet_data(&mut buf, &VERSION)
                .expect("write");
            buf
        };

        // The full packet is: VarInt count, entries..., replace bool. Its per-entry
        // flags are notification|highlight with highlight = !replace, so replace=false
        // gives highlight = true.
        let mut cursor = 0usize;
        let (total, len) = read_var_int(&full[cursor..]);
        cursor += len;
        assert!(total > 8, "expected a populated recipe table, got {total}");

        // Walk every entry by re-encoding each one alone and matching the prefix,
        // which both locates the boundary and proves the bytes are equal. Covering
        // the whole table is what catches cooking-category and group-id drift.
        for display_id in 0..total {
            let one = encode_subset(
                false,
                &[RecipeBookEntry {
                    display_id,
                    notification: show_notification(display_id),
                    highlight: true,
                }],
            );
            // Strip the leading count (0x01) and the trailing replace flag.
            let entry = &one[1..one.len() - 1];
            assert_eq!(
                &full[cursor..cursor + entry.len()],
                entry,
                "entry {display_id} differs between the full and subset writers"
            );
            cursor += entry.len();
        }
        assert_eq!(
            cursor,
            full.len() - 1,
            "the full packet must be exactly its entries plus the trailing replace flag"
        );
    }

    fn read_var_int(bytes: &[u8]) -> (i32, usize) {
        let mut value: i32 = 0;
        let mut position = 0;
        let mut index = 0;
        loop {
            let byte = bytes[index];
            value |= i32::from(byte & 0x7F) << position;
            index += 1;
            if byte & 0x80 == 0 {
                return (value, index);
            }
            position += 7;
        }
    }
}
