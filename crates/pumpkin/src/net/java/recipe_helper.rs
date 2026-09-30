use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::recipes::RecipeIngredientTypes;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_protocol::codec::recipe::OwnedRecipeIngredient;
use pumpkin_world::inventory::Inventory;

#[derive(Clone, Copy)]
pub enum GenericIngredient<'a> {
    Vanilla(&'a RecipeIngredientTypes),
    Dynamic(&'a OwnedRecipeIngredient),
}

impl GenericIngredient<'_> {
    #[must_use]
    pub fn match_item(&self, item: &Item) -> bool {
        match self {
            Self::Vanilla(v) => v.match_item(item),
            Self::Dynamic(d) => d.match_item(item),
        }
    }
}

/// Vanilla `StackedItemContents` as used by `ServerPlaceRecipe` (`StackedItemContents.java`):
/// per-item availability of the crafting-usable stacks, each stack capped at its max size.
///
/// The allocation is a greedy first-fit per ingredient, not `StackedContents.tryPick`'s
/// bipartite matching.
#[derive(Default)]
pub struct AvailableItems {
    items: Vec<(&'static Item, u32)>,
}

impl AvailableItems {
    /// `StackedItemContents.accountSimpleStack` (`:14-18`, `:20-30`).
    pub fn account_simple_stack(&mut self, stack: &ItemStack) {
        if stack.is_empty() || !PlayerInventory::is_usable_for_crafting(stack) {
            return;
        }
        let count = u32::from(stack.item_count.min(stack.get_max_stack_size()));
        if let Some(e) = self.items.iter_mut().find(|(i, _)| i.id == stack.item.id) {
            e.1 += count;
        } else {
            self.items.push((stack.item, count));
        }
    }

    /// `StackedItemContents.canCraft(recipe, amount, output)`: the item chosen for each
    /// ingredient (same order as `ingredients`), or `None` if `amount` cannot be supplied.
    #[must_use]
    pub fn plan(
        &self,
        ingredients: &[GenericIngredient<'_>],
        amount: u8,
    ) -> Option<Vec<&'static Item>> {
        let amount = u32::from(amount);
        let mut budget = self.items.clone();
        let mut chosen = Vec::with_capacity(ingredients.len());
        for ing in ingredients {
            let idx = budget
                .iter()
                .position(|(item, count)| *count >= amount && ing.match_item(item))?;
            budget[idx].1 -= amount;
            chosen.push(budget[idx].0);
        }
        Some(chosen)
    }

    /// `StackedItemContents.getBiggestCraftableStack` (bounded to one full stack here).
    #[must_use]
    pub fn biggest_craftable(&self, ingredients: &[GenericIngredient<'_>]) -> u8 {
        if ingredients.is_empty() {
            return 0;
        }
        (1u8..=64)
            .rev()
            .find(|amount| self.plan(ingredients, *amount).is_some())
            .unwrap_or(0)
    }
}

/// `ServerPlaceRecipe.clampToMaxStackSize` (`ServerPlaceRecipe.java:137-143`): the amount is
/// limited by the max stack size of every item used (default 1 without the component).
#[must_use]
pub fn clamp_to_max_stack_size(amount: u8, items: &[&'static Item]) -> u8 {
    items.iter().fold(amount, |value, item| {
        value.min(ItemStack::new(1, item).get_max_stack_size())
    })
}

/// `ServerPlaceRecipe.testClearGrid` (`ServerPlaceRecipe.java:195-228`): whether every stack in
/// the input grid could be handed back to the inventory without dropping anything.
pub async fn test_clear_grid(inventory: &PlayerInventory, grid: &[ItemStack]) -> bool {
    let free_slots_in_inventory = inventory.free_main_slot_count().await;
    let mut free_slots: Vec<ItemStack> = Vec::new();

    for grid_stack in grid {
        if grid_stack.is_empty() {
            continue;
        }
        let mut stack = grid_stack.clone();
        let slot_id = inventory.get_occupied_slot_with_room_for_stack(&stack).await;
        if slot_id == -1 && free_slots.len() <= free_slots_in_inventory {
            for listed in &mut free_slots {
                if listed.is_same_item(&stack)
                    && listed.item_count != listed.get_max_stack_size()
                    && u16::from(listed.item_count) + u16::from(stack.item_count)
                        <= u16::from(listed.get_max_stack_size())
                {
                    listed.increment(stack.item_count);
                    stack.set_count(0);
                    break;
                }
            }

            if !stack.is_empty() {
                if free_slots.len() >= free_slots_in_inventory {
                    return false;
                }
                free_slots.push(stack);
            }
        } else if slot_id == -1 {
            return false;
        }
    }
    true
}

/// `ServerPlaceRecipe.moveItemToGrid` (`ServerPlaceRecipe.java:170-193`).
///
/// Moves up to `count` of exactly `item` from the inventory into the grid slot, returning the
/// amount still missing or `None` when the inventory has no matching stack left.
pub async fn move_item_to_grid(
    inventory: &PlayerInventory,
    grid: &dyn Inventory,
    grid_index: usize,
    item: &Item,
    count: u8,
) -> Option<u8> {
    let mut in_target = grid.get_stack(grid_index).await;
    let taken = {
        let mut main_inventory = inventory.main_inventory.write().await;
        let slot = PlayerInventory::find_slot_matching_crafting_ingredient_in(
            &*main_inventory,
            item,
            &in_target,
        )?;
        let taken = main_inventory[slot].split(count);
        if main_inventory[slot].is_empty() {
            main_inventory[slot] = ItemStack::EMPTY.clone();
        }
        taken
    };

    let taken_count = taken.item_count;
    if in_target.is_empty() {
        grid.set_stack(grid_index, taken).await;
    } else {
        in_target.increment(taken_count);
        grid.set_stack(grid_index, in_target).await;
    }
    Some(count - taken_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MILK: RecipeIngredientTypes = RecipeIngredientTypes::Simple("minecraft:milk_bucket");

    #[test]
    fn only_usable_stacks_are_counted_and_capped() {
        let mut available = AvailableItems::default();
        let mut renamed = ItemStack::new(1, &Item::MILK_BUCKET);
        renamed.set_custom_name("x".to_string());
        available.account_simple_stack(&renamed);
        assert_eq!(available.biggest_craftable(&[GenericIngredient::Vanilla(&MILK)]), 0);

        available.account_simple_stack(&ItemStack::new(1, &Item::MILK_BUCKET));
        available.account_simple_stack(&ItemStack::new(1, &Item::MILK_BUCKET));
        // Each milk bucket stack is capped at its max stack size of 1.
        assert_eq!(available.biggest_craftable(&[GenericIngredient::Vanilla(&MILK)]), 2);
        let three = [GenericIngredient::Vanilla(&MILK); 3];
        assert_eq!(available.biggest_craftable(&three), 0);
    }

    #[test]
    fn amount_is_clamped_to_smallest_max_stack_size() {
        assert_eq!(
            clamp_to_max_stack_size(64, &[&Item::MILK_BUCKET, &Item::SUGAR]),
            1
        );
        assert_eq!(clamp_to_max_stack_size(5, &[&Item::SUGAR]), 5);
    }
}
