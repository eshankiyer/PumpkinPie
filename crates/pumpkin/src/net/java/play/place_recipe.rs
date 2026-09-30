#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    #[allow(clippy::too_many_lines)]
    pub async fn handle_place_recipe(
        &self,
        server: &Arc<Server>,
        player: &Arc<Player>,
        packet: SPlaceRecipe,
    ) {
        use crate::net::java::recipe_helper::{
            AvailableItems, GenericIngredient, clamp_to_max_stack_size, move_item_to_grid,
            test_clear_grid,
        };
        use crate::server::recipe::DynamicRecipe;
        use pumpkin_data::recipes::{CraftingRecipeTypes, RECIPES_COOKING, RECIPES_CRAFTING};
        use pumpkin_data::screen::WindowType;
        use pumpkin_inventory::crafting::recipe_provider::RecipeProvider;

        let target_id = packet.recipe_display_id.0 as usize;
        let use_max = packet.use_max_items;

        let mut click_event = crate::plugin::api::events::player::player_recipe_book_click::PlayerRecipeBookClickEvent::new(
            player.clone(),
            format!("display_{}", packet.recipe_display_id.0),
            use_max,
        );
        server.plugin_manager.fire(server, &mut click_event).await;
        if click_event.cancelled {
            return;
        }

        // Count crafting display IDs.
        let crafting_display_count = RECIPES_CRAFTING
            .iter()
            .filter(|r| {
                !matches!(
                    r,
                    CraftingRecipeTypes::CraftingSpecial
                        | CraftingRecipeTypes::CraftingDecoratedPot { .. }
                )
            })
            .count();
        let cooking_display_count = RECIPES_COOKING.len();
        let stonecutting_display_count = pumpkin_data::recipes::RECIPES_STONECUTTING.len();
        let dynamic_recipes = server.recipe_manager.get_dynamic_recipes().await;

        let (grid_width, crafting_inv) = {
            let screen_handler_arc = player.current_screen_handler.lock().await.clone();
            let handler = screen_handler_arc.lock().await;
            let grid_width: usize = match handler.window_type() {
                Some(WindowType::Crafting) => 3,
                None => 2, // player inventory 2x2
                _ => return,
            };
            (grid_width, handler.get_behaviour().slots[1].get_inventory())
        };

        let grid_size = grid_width * grid_width;
        let mut ingredient_slots: Vec<Option<GenericIngredient<'_>>> = vec![None; grid_size];
        let ghost_source;

        if target_id < crafting_display_count {
            // Crafting recipe
            let mut counter = 0usize;
            let recipe = RECIPES_CRAFTING.iter().find(|r| {
                if matches!(
                    r,
                    CraftingRecipeTypes::CraftingSpecial
                        | CraftingRecipeTypes::CraftingDecoratedPot { .. }
                ) {
                    return false;
                }
                let found = counter == target_id;
                counter += 1;
                found
            });
            let Some(recipe) = recipe else { return };
            ghost_source =
                Some(pumpkin_protocol::java::client::play::GhostRecipeSource::Static(recipe));

            match recipe {
                CraftingRecipeTypes::CraftingShaped { pattern, key, .. } => {
                    for (row, row_str) in pattern.iter().enumerate() {
                        for (col, ch) in row_str.chars().enumerate() {
                            if ch != ' '
                                && let Some(ing) =
                                    key.iter().find_map(|(k, v)| (*k == ch).then_some(v))
                                && row * grid_width + col < grid_size
                            {
                                ingredient_slots[row * grid_width + col] =
                                    Some(GenericIngredient::Vanilla(ing));
                            }
                        }
                    }
                }
                CraftingRecipeTypes::CraftingShapeless { ingredients, .. } => {
                    for (i, ing) in ingredients.iter().enumerate().take(grid_size) {
                        ingredient_slots[i] = Some(GenericIngredient::Vanilla(ing));
                    }
                }
                CraftingRecipeTypes::CraftingTransmute {
                    input, material, ..
                } => {
                    if grid_size >= 2 {
                        ingredient_slots[0] = Some(GenericIngredient::Vanilla(input));
                        ingredient_slots[1] = Some(GenericIngredient::Vanilla(material));
                    }
                }
                CraftingRecipeTypes::CraftingDye { target, dye, .. } => {
                    if grid_size >= 2 {
                        ingredient_slots[0] = Some(GenericIngredient::Vanilla(target));
                        ingredient_slots[1] = Some(GenericIngredient::Vanilla(dye));
                    }
                }
                CraftingRecipeTypes::CraftingImbue {
                    source, material, ..
                } => {
                    if grid_width != 3 {
                        return;
                    }
                    ingredient_slots.fill(Some(GenericIngredient::Vanilla(material)));
                    ingredient_slots[4] = Some(GenericIngredient::Vanilla(source));
                }
                _ => return,
            }
        } else if target_id < crafting_display_count + cooking_display_count {
            // TODO: cooking recipes
            return;
        } else if target_id
            < crafting_display_count + cooking_display_count + stonecutting_display_count
        {
            // Vanilla `StonecutterRecipe.display()` entries use the shared recipe-book display
            // id stream (`StonecutterRecipe.java:37-49`); stonecutter selection is handled by
            // its screen button rather than this crafting-grid placement path.
            return;
        } else {
            let dynamic_id = target_id
                - crafting_display_count
                - cooking_display_count
                - stonecutting_display_count;
            let Some(DynamicRecipe::Crafting(crafting)) = dynamic_recipes.get(dynamic_id) else {
                return;
            };
            ghost_source =
                Some(pumpkin_protocol::java::client::play::GhostRecipeSource::Dynamic(crafting));

            match crafting {
                pumpkin_protocol::codec::recipe::OwnedCraftingRecipe::Shaped {
                    pattern,
                    key,
                    ..
                } => {
                    for (row, row_str) in pattern.iter().enumerate() {
                        for (col, ch) in row_str.chars().enumerate() {
                            if ch != ' '
                                && let Some((_, ing)) = key.iter().find(|(k, _)| *k == ch)
                                && row * grid_width + col < grid_size
                            {
                                ingredient_slots[row * grid_width + col] =
                                    Some(GenericIngredient::Dynamic(ing));
                            }
                        }
                    }
                }

                pumpkin_protocol::codec::recipe::OwnedCraftingRecipe::Shapeless {
                    ingredients,
                    ..
                } => {
                    for (i, ing) in ingredients.iter().enumerate().take(grid_size) {
                        ingredient_slots[i] = Some(GenericIngredient::Dynamic(ing));
                    }
                }
                pumpkin_protocol::codec::recipe::OwnedCraftingRecipe::Dye {
                    target, dye, ..
                } => {
                    if grid_size >= 2 {
                        ingredient_slots[0] = Some(GenericIngredient::Dynamic(target));
                        ingredient_slots[1] = Some(GenericIngredient::Dynamic(dye));
                    }
                }
                pumpkin_protocol::codec::recipe::OwnedCraftingRecipe::Imbue {
                    source,
                    material,
                    ..
                } => {
                    if grid_width != 3 {
                        return;
                    }
                    ingredient_slots.fill(Some(GenericIngredient::Dynamic(material)));
                    ingredient_slots[4] = Some(GenericIngredient::Dynamic(source));
                }
            }
        }

        // Current grid contents (vanilla reads the `inputGridSlots` directly).
        let mut grid_stacks = Vec::with_capacity(grid_size);
        for idx in 0..grid_size {
            grid_stacks.push(crafting_inv.get_stack(idx).await);
        }

        // `ServerPlaceRecipe.placeRecipe` (`ServerPlaceRecipe.java:36-42`): unless the player can
        // drop items (creative), a grid that cannot be handed back to the inventory changes nothing.
        if !player.is_creative() && !test_clear_grid(&player.inventory, &grid_stacks).await {
            return;
        }

        // `inventory.fillStackedContents` + `menu.fillCraftSlotsStackedContents`.
        let mut available = AvailableItems::default();
        for stack in player.inventory.main_inventory.read().await.iter() {
            available.account_simple_stack(stack);
        }
        for stack in &grid_stacks {
            available.account_simple_stack(stack);
        }

        let active_ingredients: Vec<GenericIngredient<'_>> =
            ingredient_slots.iter().flatten().copied().collect();

        // Vanilla `ServerPlaceRecipe.tryPlaceRecipe`: if the player can't craft the recipe
        // at all, the grid is cleared and the client is told to render the recipe as a ghost
        // overlay instead.
        if available.plan(&active_ingredients, 1).is_none() {
            for (i, stack) in grid_stacks.iter().enumerate() {
                if !stack.is_empty() {
                    crafting_inv.remove_stack(i).await;
                    player
                        .inventory
                        .offer(stack.clone(), false, player.as_ref())
                        .await;
                }
            }
            if let Some(source) = ghost_source {
                self.enqueue_client_packet(
                    &pumpkin_protocol::java::client::play::CPlaceGhostRecipe::new(
                        VarInt(i32::from(
                            player
                                .current_screen_handler
                                .lock()
                                .await
                                .lock()
                                .await
                                .sync_id(),
                        )),
                        source,
                    ),
                )
                .await;
            }
            let screen_handler_arc = player.current_screen_handler.lock().await.clone();
            screen_handler_arc.lock().await.send_content_updates().await;
            return;
        }

        // Check if this exact recipe is already placed (determines stacking vs fresh fill).
        let recipe_matches = ingredient_slots.iter().zip(&grid_stacks).all(|(ing, stack)| {
            ing.as_ref().map_or_else(
                || stack.is_empty(),
                |ingredient| !stack.is_empty() && ingredient.match_item(stack.item),
            )
        });

        let biggest_craftable = available.biggest_craftable(&active_ingredients);

        // `ServerPlaceRecipe.placeRecipe` (`:96-101`): a placed recipe whose stacks cannot grow
        // any further is left untouched.
        if recipe_matches
            && grid_stacks.iter().any(|stack| {
                !stack.is_empty()
                    && biggest_craftable.min(stack.get_max_stack_size()) < stack.item_count.saturating_add(1)
            })
        {
            return;
        }

        // `ServerPlaceRecipe.calculateAmountToCraft` (`:145-168`).
        let amount_to_craft = if use_max {
            biggest_craftable
        } else if recipe_matches {
            grid_stacks
                .iter()
                .filter(|stack| !stack.is_empty())
                .map(|stack| stack.item_count)
                .min()
                .map_or(1, |min| min.saturating_add(1))
        } else {
            1
        };

        let Some(mut items_used) = available.plan(&active_ingredients, amount_to_craft) else {
            return;
        };
        let adjusted_amount = clamp_to_max_stack_size(amount_to_craft, &items_used);
        if adjusted_amount != amount_to_craft {
            let Some(items) = available.plan(&active_ingredients, adjusted_amount) else {
                return;
            };
            items_used = items;
        }

        // `clearGrid`: hand the grid back to the inventory (`placeItemBackInInventory`).
        for (i, stack) in grid_stacks.iter().enumerate() {
            if !stack.is_empty() {
                crafting_inv.remove_stack(i).await;
                player
                    .inventory
                    .offer(stack.clone(), false, player.as_ref())
                    .await;
            }
        }

        // Fill each grid slot with `adjusted_amount` of the item chosen for its ingredient.
        let mut ingredient_index = 0usize;
        for (idx, ing) in ingredient_slots.iter().enumerate() {
            if ing.is_none() {
                continue;
            }
            let item = items_used[ingredient_index];
            ingredient_index += 1;
            let mut remaining = adjusted_amount;
            while remaining > 0 {
                let Some(left) = move_item_to_grid(
                    &player.inventory,
                    crafting_inv.as_ref(),
                    idx,
                    item,
                    remaining,
                )
                .await
                else {
                    break;
                };
                remaining = left;
            }
        }

        let screen_handler_arc = player.current_screen_handler.lock().await.clone();
        screen_handler_arc.lock().await.send_content_updates().await;
    }
}
