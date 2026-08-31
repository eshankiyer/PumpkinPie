use pumpkin_inventory::crafting::recipe_provider::RecipeProvider;
use pumpkin_inventory::slot::BoxFuture;
pub use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_protocol::codec::recipe::{OwnedCookingRecipeType, OwnedCraftingRecipe};
use tokio::sync::RwLock;

pub struct RecipeManager {
    dynamic_recipes: RwLock<Vec<DynamicRecipe>>,
}

impl Default for RecipeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RecipeManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            dynamic_recipes: RwLock::new(Vec::new()),
        }
    }

    pub async fn add_recipe(&self, recipe: DynamicRecipe) {
        let mut recipes = self.dynamic_recipes.write().await;
        recipes.push(recipe);
    }

    pub async fn add_recipes(&self, new_recipes: impl IntoIterator<Item = DynamicRecipe>) {
        let mut recipes = self.dynamic_recipes.write().await;
        recipes.extend(new_recipes);
    }

    pub async fn set_recipes(&self, new_recipes: Vec<DynamicRecipe>) {
        let mut recipes = self.dynamic_recipes.write().await;
        *recipes = new_recipes;
    }

    pub async fn clear(&self) {
        let mut recipes = self.dynamic_recipes.write().await;
        recipes.clear();
    }

    pub async fn get_dynamic_recipes_internal(&self) -> Vec<DynamicRecipe> {
        self.dynamic_recipes.read().await.clone()
    }

    // RecipeManager.java:175-177 exposes every loaded recipe to server callers; combine the
    // generated vanilla registry with datapack/plugin recipes for the live recipe command.
    pub async fn get_recipe_ids(&self) -> Vec<String> {
        let registry = crate::data::recipe_book::registry();
        let mut ids = (0..registry.len())
            .map(|index| registry.id_of(index).to_owned())
            .collect::<Vec<_>>();
        let dynamic = self.dynamic_recipes.read().await;
        ids.extend(dynamic.iter().map(dynamic_recipe_id));
        ids
    }
}

fn dynamic_recipe_id(recipe: &DynamicRecipe) -> String {
    match recipe {
        DynamicRecipe::Crafting(crafting) => match crafting {
            OwnedCraftingRecipe::Shaped {
                recipe_id, result, ..
            }
            | OwnedCraftingRecipe::Shapeless {
                recipe_id, result, ..
            }
            | OwnedCraftingRecipe::Dye {
                recipe_id, result, ..
            }
            | OwnedCraftingRecipe::Imbue {
                recipe_id, result, ..
            } => recipe_id.clone().unwrap_or_else(|| result.item_id.clone()),
        },
        DynamicRecipe::Cooking(cooking) => match cooking {
            OwnedCookingRecipeType::Smelting(recipe)
            | OwnedCookingRecipeType::Blasting(recipe)
            | OwnedCookingRecipeType::Smoking(recipe)
            | OwnedCookingRecipeType::CampfireCooking(recipe) => recipe.recipe_id.clone(),
        },
    }
}

impl RecipeProvider for RecipeManager {
    fn get_dynamic_recipes(&self) -> BoxFuture<'_, Vec<DynamicRecipe>> {
        Box::pin(async move { self.dynamic_recipes.read().await.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::recipes::RecipeCategoryTypes;
    use pumpkin_protocol::codec::recipe::{OwnedRecipeIngredient, OwnedRecipeResult};

    #[tokio::test]
    async fn recipe_ids_include_generated_and_dynamic_recipes() {
        // RecipeManager.java:175-177 requires the query to include both recipe sources.
        let manager = RecipeManager::new();
        manager
            .add_recipe(DynamicRecipe::Crafting(OwnedCraftingRecipe::Shapeless {
                recipe_id: Some("example:custom_recipe".to_owned()),
                category: RecipeCategoryTypes::Misc,
                group: None,
                ingredients: vec![OwnedRecipeIngredient::Simple("minecraft:stone".to_owned())],
                result: OwnedRecipeResult {
                    item_id: "minecraft:stone".to_owned(),
                    count: 1,
                },
            }))
            .await;

        let ids = manager.get_recipe_ids().await;
        assert!(ids.iter().any(|id| id == "example:custom_recipe"));
        assert!(
            ids.iter()
                .any(|id| id == crate::data::recipe_book::registry().id_of(0))
        );
    }
}
