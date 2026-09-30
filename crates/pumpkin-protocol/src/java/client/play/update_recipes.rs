use std::io::Write;

use crate::{
    ClientPacket, VarInt, WritingError,
    codec::recipe::{DynamicRecipe, OwnedCookingRecipeType, OwnedRecipeIngredient},
    ser::NetworkWriteExt,
};
use pumpkin_data::item::Item;
use pumpkin_data::packet::clientbound::play::UPDATE_RECIPES;
use pumpkin_data::recipes::{
    CookingRecipeType, RECIPES_COOKING, RECIPES_STONECUTTING, RecipeIngredientTypes,
};
use pumpkin_data::smithing::SMITHING_TRANSFORM_RECIPES;
use pumpkin_data::trim::TrimPattern;
use pumpkin_macros::java_packet;
use pumpkin_util::version::JavaMinecraftVersion;

use super::recipe_book_add::{
    item_id_versioned, resolve_item_tag, write_ingredient_holderset, write_result_slot_display,
};

/// `ClientboundUpdateRecipesPacket`.
///
/// Carries the `RecipePropertySet` item sets for furnace and smithing slots plus the
/// stonecutter button list (`SelectableRecipe.SingleInputSet`, sent without the recipe holder).
///
/// Everything is derived at write time from the generated recipe tables, plus the cooking
/// recipes in `dynamic_recipes`, since ids and tag contents differ per protocol version.
#[java_packet(UPDATE_RECIPES)]
pub struct CUpdateRecipes<'a> {
    pub dynamic_recipes: &'a [DynamicRecipe],
}

impl<'a> CUpdateRecipes<'a> {
    #[must_use]
    pub const fn new(dynamic_recipes: &'a [DynamicRecipe]) -> Self {
        Self { dynamic_recipes }
    }
}

fn push_unique(set: &mut Vec<&'static Item>, items: impl IntoIterator<Item = &'static Item>) {
    for item in items {
        if !set.iter().any(|existing| existing.id == item.id) {
            set.push(item);
        }
    }
}

fn named_items<'n>(
    names: impl IntoIterator<Item = &'n str>,
) -> impl Iterator<Item = &'static Item> {
    names.into_iter().filter_map(|name| {
        Item::from_registry_key(name.strip_prefix("minecraft:").unwrap_or(name))
    })
}

fn tag_items(tag: &str, version: JavaMinecraftVersion) -> Vec<&'static Item> {
    resolve_item_tag(tag, version).unwrap_or_default()
}

fn static_ingredient_items(
    ingredient: &RecipeIngredientTypes,
    version: JavaMinecraftVersion,
) -> Vec<&'static Item> {
    match ingredient {
        RecipeIngredientTypes::Simple(name) => named_items([*name]).collect(),
        RecipeIngredientTypes::Tagged(tag) => tag_items(tag, version),
        RecipeIngredientTypes::OneOf(names) => named_items(names.iter().copied()).collect(),
    }
}

fn owned_ingredient_items(
    ingredient: &OwnedRecipeIngredient,
    version: JavaMinecraftVersion,
) -> Vec<&'static Item> {
    match ingredient {
        OwnedRecipeIngredient::Simple(name) => named_items([name.as_str()]).collect(),
        OwnedRecipeIngredient::Tagged(tag) => tag_items(tag, version),
        OwnedRecipeIngredient::OneOf(names) => {
            named_items(names.iter().map(String::as_str)).collect()
        }
    }
}

/// The seven `RecipeManager.RECIPE_PROPERTY_SETS` keys, each always present (empty when no
/// recipe contributes), as flat unions of the items of the matching ingredients.
struct PropertySets {
    smithing_base: Vec<&'static Item>,
    smithing_template: Vec<&'static Item>,
    smithing_addition: Vec<&'static Item>,
    furnace_input: Vec<&'static Item>,
    blast_furnace_input: Vec<&'static Item>,
    smoker_input: Vec<&'static Item>,
    campfire_input: Vec<&'static Item>,
}

impl PropertySets {
    fn collect(dynamic_recipes: &[DynamicRecipe], version: JavaMinecraftVersion) -> Self {
        let mut sets = Self {
            smithing_base: Vec::new(),
            smithing_template: Vec::new(),
            smithing_addition: Vec::new(),
            furnace_input: Vec::new(),
            blast_furnace_input: Vec::new(),
            smoker_input: Vec::new(),
            campfire_input: Vec::new(),
        };

        // Smithing recipes are data-driven in vanilla; the server's own tables are the
        // netherite upgrades and one trim recipe per pattern (see `pumpkin_data::smithing`).
        push_unique(
            &mut sets.smithing_base,
            SMITHING_TRANSFORM_RECIPES.iter().map(|recipe| recipe.base),
        );
        push_unique(
            &mut sets.smithing_base,
            tag_items("minecraft:trimmable_armor", version),
        );
        push_unique(
            &mut sets.smithing_template,
            std::iter::once(&Item::NETHERITE_UPGRADE_SMITHING_TEMPLATE)
                .chain(TrimPattern::ALL.into_iter().map(TrimPattern::template_item)),
        );
        push_unique(
            &mut sets.smithing_addition,
            tag_items("minecraft:netherite_tool_materials", version),
        );
        push_unique(
            &mut sets.smithing_addition,
            tag_items("minecraft:trim_materials", version),
        );

        for recipe in RECIPES_COOKING {
            let (set, cooking) = match recipe {
                CookingRecipeType::Smelting(r) => (&mut sets.furnace_input, r),
                CookingRecipeType::Blasting(r) => (&mut sets.blast_furnace_input, r),
                CookingRecipeType::Smoking(r) => (&mut sets.smoker_input, r),
                CookingRecipeType::CampfireCooking(r) => (&mut sets.campfire_input, r),
            };
            push_unique(set, static_ingredient_items(&cooking.ingredient, version));
        }
        for recipe in dynamic_recipes {
            let DynamicRecipe::Cooking(cooking) = recipe else {
                continue;
            };
            let (set, cooking) = match cooking {
                OwnedCookingRecipeType::Smelting(r) => (&mut sets.furnace_input, r),
                OwnedCookingRecipeType::Blasting(r) => (&mut sets.blast_furnace_input, r),
                OwnedCookingRecipeType::Smoking(r) => (&mut sets.smoker_input, r),
                OwnedCookingRecipeType::CampfireCooking(r) => (&mut sets.campfire_input, r),
            };
            push_unique(set, owned_ingredient_items(&cooking.ingredient, version));
        }
        sets
    }

    fn entries(&self) -> [(&'static str, &[&'static Item]); 7] {
        [
            ("minecraft:smithing_base", &self.smithing_base),
            ("minecraft:smithing_template", &self.smithing_template),
            ("minecraft:smithing_addition", &self.smithing_addition),
            ("minecraft:furnace_input", &self.furnace_input),
            ("minecraft:blast_furnace_input", &self.blast_furnace_input),
            ("minecraft:smoker_input", &self.smoker_input),
            ("minecraft:campfire_input", &self.campfire_input),
        ]
    }
}

fn write_item_sets(
    write: &mut impl Write,
    sets: &PropertySets,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let entries = sets.entries();
    write.write_var_int(&VarInt(entries.len() as i32))?;
    for (key, items) in entries {
        write.write_string(key)?;
        write.write_var_int(&VarInt(items.len() as i32))?;
        for item in items {
            write.write_var_int(&VarInt(item_id_versioned(item, version)))?;
        }
    }
    Ok(())
}

impl ClientPacket for CUpdateRecipes<'_> {
    fn write_packet_data(
        &self,
        write: impl Write,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let mut write = write;
        let version = *version;

        write_item_sets(
            &mut write,
            &PropertySets::collect(self.dynamic_recipes, version),
            version,
        )?;

        // SingleInputSet.noRecipeCodec: list of { Ingredient contents, SlotDisplay }, in the
        // recipe-id order `RecipeManager.finalizeRecipeLoading` walks (the client's button order).
        write.write_var_int(&VarInt(RECIPES_STONECUTTING.len() as i32))?;
        for recipe in RECIPES_STONECUTTING {
            write_ingredient_holderset(&mut write, &recipe.ingredient, version)?;
            write_result_slot_display(&mut write, &recipe.result, version)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn furnace_and_smithing_sets_are_populated_and_deduplicated() {
        let sets = PropertySets::collect(&[], JavaMinecraftVersion::V_26_2);
        assert!(sets.furnace_input.iter().any(|i| i.id == Item::IRON_ORE.id));
        assert!(sets.campfire_input.iter().any(|i| i.id == Item::BEEF.id));
        assert!(
            sets.smithing_template
                .iter()
                .any(|i| i.id == Item::NETHERITE_UPGRADE_SMITHING_TEMPLATE.id)
        );
        assert_eq!(sets.smithing_template.len(), 1 + TrimPattern::ALL.len());
        let mut ids: Vec<u16> = sets.furnace_input.iter().map(|i| i.id).collect();
        let len = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), len);
    }
}
