use std::any::Any;
use std::sync::Arc;

use pumpkin_data::Enchantment;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    DataComponentImpl, EnchantableImpl, StoredEnchantmentsImpl,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::screen::WindowType;
use pumpkin_data::statistic::{CustomStatistic, StatisticCategory};
use pumpkin_data::tag::{Enchantment as EnchantmentTag, Taggable};
use pumpkin_util::random::{RandomImpl, legacy_rand::LegacyRand};
use pumpkin_world::inventory::Inventory;

use crate::{
    player::player_inventory::PlayerInventory,
    screen_handler::{
        InventoryPlayer, ItemStackFuture, ScreenHandler, ScreenHandlerBehaviour,
        ScreenHandlerFuture, offer_or_drop_stack,
    },
    slot::{BoxFuture, NormalSlot, Slot},
    window_property::{EnchantmentTable, WindowProperty},
};

struct LapisSlot(NormalSlot);

fn is_lapis(stack: &ItemStack) -> bool {
    stack.item == &Item::LAPIS_LAZULI
}

/// `ItemStack.enchant` routes through `EnchantmentHelper.getComponentType`
/// (EnchantmentHelper.java:81-83): an enchanted book stores its levels in
/// `StoredEnchantments`, every other item in `Enchantments`.
fn add_enchantment_for(stack: &mut ItemStack, enchantment: &'static Enchantment, level: i32) {
    if stack.item != &Item::ENCHANTED_BOOK {
        stack.add_enchantment(enchantment, level as u16);
        return;
    }
    let mut stored = stack
        .get_data_component::<StoredEnchantmentsImpl>()
        .map(|c| c.enchantment.to_vec())
        .unwrap_or_default();
    stored.push((enchantment, level));
    stack
        .patch
        .retain(|(id, _)| *id != DataComponent::StoredEnchantments);
    let boxed: Box<dyn DataComponentImpl> = Box::new(StoredEnchantmentsImpl {
        enchantment: stored.into(),
    });
    stack
        .patch
        .push((DataComponent::StoredEnchantments, Some(boxed)));
}

impl LapisSlot {
    fn new(inventory: Arc<dyn Inventory>) -> Self {
        Self(NormalSlot::new(inventory, 1))
    }
}

impl Slot for LapisSlot {
    fn get_inventory(&self) -> Arc<dyn Inventory> {
        self.0.get_inventory()
    }

    fn get_index(&self) -> usize {
        self.0.get_index()
    }

    fn set_id(&self, id: usize) {
        self.0.set_id(id);
    }

    fn can_insert<'a>(&'a self, stack: &'a ItemStack) -> BoxFuture<'a, bool> {
        Box::pin(async move { is_lapis(stack) })
    }

    fn mark_dirty(&self) -> BoxFuture<'_, ()> {
        self.0.mark_dirty()
    }
}

pub struct EnchantingTableScreenHandler {
    pub inventory: Arc<dyn Inventory>,
    behaviour: ScreenHandlerBehaviour,
    pub level_requirements: [i32; 3],
    pub enchantment_id: [i32; 3],
    pub enchantment_level: [i32; 3],
    pub enchantment_seed: i32,
    pub bookshelf_count: i32,
}

impl EnchantingTableScreenHandler {
    pub fn new(
        sync_id: u8,
        player_inventory: &Arc<PlayerInventory>,
        inventory: &Arc<dyn Inventory>,
        enchantment_seed: i32,
        bookshelf_count: i32,
    ) -> Self {
        let mut handler = Self {
            inventory: inventory.clone(),
            behaviour: ScreenHandlerBehaviour::new(sync_id, Some(WindowType::Enchantment)),
            level_requirements: [0; 3],
            enchantment_id: [-1; 3],
            enchantment_level: [-1; 3],
            enchantment_seed,
            bookshelf_count,
        };

        // Enchanting slots: 0 is item, 1 is lapis
        handler.add_slot(Arc::new(NormalSlot::new(inventory.clone(), 0)));
        handler.add_slot(Arc::new(LapisSlot::new(inventory.clone())));

        let player_inventory: Arc<dyn Inventory> = player_inventory.clone();
        handler.add_player_slots(&player_inventory);

        handler
    }

    pub async fn update_enchantments(&mut self, player: &dyn InventoryPlayer) {
        let item = self.inventory.get_stack(0).await;

        if item.is_empty() || item.has_enchantments() {
            for i in 0..3 {
                self.level_requirements[i] = 0;
                self.enchantment_id[i] = -1;
                self.enchantment_level[i] = -1;
            }
        } else {
            let enchantability = item
                .get_data_component::<EnchantableImpl>()
                .map_or(0, |e| e.value);

            if enchantability <= 0 {
                for i in 0..3 {
                    self.level_requirements[i] = 0;
                    self.enchantment_id[i] = -1;
                    self.enchantment_level[i] = -1;
                }
            } else {
                let mut random = LegacyRand::from_seed(self.enchantment_seed as u64);

                for i in 0..3 {
                    let mut level =
                        self.calculate_level_requirement(&mut random, i, enchantability);
                    // EnchantmentMenu.java:106-107 (26.2 decompile): a slot whose computed cost is
                    // below its own lapis price (slot index + 1) is unavailable, not just cheap.
                    if level < i as i32 + 1 {
                        level = 0;
                    }
                    self.level_requirements[i] = level;
                }

                for i in 0..3 {
                    if self.level_requirements[i] > 0 {
                        let mut random = self.create_enchantment_random(i);
                        let enchantments = Self::get_enchantment_list(
                            &mut random,
                            &item,
                            i,
                            self.level_requirements[i],
                        );
                        if enchantments.is_empty() {
                            self.enchantment_id[i] = -1;
                            self.enchantment_level[i] = -1;
                        } else {
                            let clue_index =
                                random.next_bounded_i32(enchantments.len() as i32) as usize;
                            let clue = enchantments[clue_index];
                            self.enchantment_id[i] = clue.0.id as i32;
                            self.enchantment_level[i] = clue.1;
                        }
                    } else {
                        self.enchantment_id[i] = -1;
                        self.enchantment_level[i] = -1;
                    }
                }

                if player
                    .fire_prepare_item_enchant_event(
                        &item,
                        &mut self.level_requirements,
                        &mut self.enchantment_id,
                        &mut self.enchantment_level,
                        self.bookshelf_count,
                    )
                    .await
                {
                    for i in 0..3 {
                        self.level_requirements[i] = 0;
                        self.enchantment_id[i] = -1;
                        self.enchantment_level[i] = -1;
                    }
                }
            }
        }
        self.send_property_updates().await;
    }

    fn calculate_level_requirement(
        &self,
        random: &mut LegacyRand,
        slot: usize,
        _enchantability: i32,
    ) -> i32 {
        let b = self.bookshelf_count;
        let level = random.next_bounded_i32(8) + 1 + (b >> 1) + random.next_bounded_i32(b + 1);

        match slot {
            0 => (level / 3).max(1),
            1 => (level * 2 / 3 + 1).max(1),
            2 => level.max(b * 2).max(1),
            _ => 0,
        }
    }

    const fn create_enchantment_random(&self, slot: usize) -> LegacyRand {
        LegacyRand::from_seed(self.enchantment_seed.wrapping_add(slot as i32) as u64)
    }

    fn get_enchantment_list(
        random: &mut LegacyRand,
        item: &ItemStack,
        _slot: usize,
        level: i32,
    ) -> Vec<(&'static Enchantment, i32)> {
        let enchantability = item
            .get_data_component::<EnchantableImpl>()
            .map_or(0, |e| e.value);
        let mut enchant_level = level
            + 1
            + random.next_bounded_i32(enchantability / 4 + 1)
            + random.next_bounded_i32(enchantability / 4 + 1);
        let bonus = (random.next_f32() + random.next_f32() - 1.0) * 0.15;
        enchant_level = (enchant_level as f32 * (1.0 + bonus)).round() as i32;
        enchant_level = enchant_level.max(1);

        // `EnchantmentHelper.getAvailableEnchantmentResults` (`EnchantmentHelper.java:597-601`)
        // filters on `isPrimaryItem(stack) || isBook`, NOT on `supported_items`: five
        // enchantments declare a narrower `primary_items` set (thorns, sharpness, smite,
        // bane_of_arthropods, fire_aspect), so a table must never offer e.g. Thorns on boots
        // even though an anvil may apply it there. A plain book belongs to none of the
        // `enchantable/*` tags, so without the book bypass the candidate list is always empty
        // and a book can never be enchanted at a table at all.
        let is_book = item.item == &Item::BOOK;
        let mut available = Vec::new();
        for enchant in Enchantment::all() {
            if enchant.has_tag(&EnchantmentTag::MINECRAFT_IN_ENCHANTING_TABLE)
                && (is_book || enchant.is_primary_item(item.item))
            {
                for l in (1..=enchant.max_level).rev() {
                    if enchant_level >= enchant.min_cost.calculate(l)
                        && enchant_level <= enchant.max_cost.calculate(l)
                    {
                        available.push((*enchant, l));
                        break;
                    }
                }
            }
        }

        if available.is_empty() {
            return Vec::new();
        }

        let total_weight: i32 = available.iter().map(|(e, _)| e.weight).sum();
        if total_weight <= 0 {
            return Vec::new();
        }

        let mut weight = random.next_bounded_i32(total_weight);
        let mut selected = None;
        for (e, l) in &available {
            weight -= e.weight;
            if weight < 0 {
                selected = Some((*e, *l));
                break;
            }
        }

        let mut result = Vec::new();
        if let Some(s) = selected {
            result.push(s);

            // EnchantmentHelper.java:566-577: the loop condition compares against the running
            // `enchantmentCost` directly (halved only after each successful pick), not a
            // pre-halved value.
            let mut current_level = enchant_level;
            while random.next_bounded_i32(50) <= current_level {
                available.retain(|(e, _)| {
                    for (se, _) in &result {
                        if !e.are_compatible(se) {
                            return false;
                        }
                    }
                    true
                });

                if available.is_empty() {
                    break;
                }

                let total_weight: i32 = available.iter().map(|(e, _)| e.weight).sum();
                let mut weight = random.next_bounded_i32(total_weight);
                for (e, l) in &available {
                    weight -= e.weight;
                    if weight < 0 {
                        result.push((*e, *l));
                        break;
                    }
                }
                current_level /= 2;
            }
        }

        // EnchantmentMenu.java:194-197 (private getEnchantmentList, used for both the UI clue and
        // the applied roll): a plain book with 2+ results has one dropped at random so a book
        // never shows/grants the full roll the way a tool would.
        if item.item == &Item::BOOK && result.len() > 1 {
            let drop = random.next_bounded_i32(result.len() as i32) as usize;
            result.remove(drop);
        }

        result
    }

    async fn send_property_updates(&self) {
        if let Some(sync_handler) = self.behaviour.sync_handler.as_ref() {
            for i in 0..3 {
                let (id, val) = WindowProperty::new(
                    EnchantmentTable::LevelRequirement { slot: i as u8 },
                    self.level_requirements[i] as i16,
                )
                .into_tuple();
                sync_handler
                    .update_property(&self.behaviour, id as i32, val as i32)
                    .await;

                let (id, val) = WindowProperty::new(
                    EnchantmentTable::EnchantmentId { slot: i as u8 },
                    self.enchantment_id[i] as i16,
                )
                .into_tuple();
                sync_handler
                    .update_property(&self.behaviour, id as i32, val as i32)
                    .await;

                let (id, val) = WindowProperty::new(
                    EnchantmentTable::EnchantmentLevel { slot: i as u8 },
                    self.enchantment_level[i] as i16,
                )
                .into_tuple();
                sync_handler
                    .update_property(&self.behaviour, id as i32, val as i32)
                    .await;
            }

            let (id, val) = WindowProperty::new(
                EnchantmentTable::EnchantmentSeed,
                (self.enchantment_seed & 0xFFFF) as i16,
            )
            .into_tuple();
            sync_handler
                .update_property(&self.behaviour, id as i32, val as i32)
                .await;
        }
    }
}

impl ScreenHandler for EnchantingTableScreenHandler {
    /// Port of `EnchantmentMenu.java:218-220`: the block at the opening position must still be
    /// `Blocks.ENCHANTING_TABLE` and the player must still be within
    /// `blockInteractionRange() + 4.0` (`AbstractContainerMenu.java:93-95`).
    fn container_access(&self) -> crate::screen_handler::ContainerAccess {
        crate::screen_handler::ContainerAccess::Block(|block| {
            block.id == pumpkin_data::Block::ENCHANTING_TABLE.id
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_behaviour(&self) -> &ScreenHandlerBehaviour {
        &self.behaviour
    }

    fn get_behaviour_mut(&mut self) -> &mut ScreenHandlerBehaviour {
        &mut self.behaviour
    }

    fn on_closed<'a>(&'a mut self, player: &'a dyn InventoryPlayer) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.default_on_closed(player).await;
            self.inventory.on_close().await;
            // Return items to player
            for i in 0..2 {
                let stack = self.inventory.remove_stack(i).await;
                if !stack.is_empty() {
                    offer_or_drop_stack(player, stack).await;
                }
            }
        })
    }

    fn on_button_click<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer,
        id: i32,
    ) -> ScreenHandlerFuture<'a, bool> {
        Box::pin(async move {
            if !(0..3).contains(&id) {
                return false;
            }

            let level_req = self.level_requirements[id as usize];
            // `EnchantmentMenu.clickMenuButton` (EnchantmentMenu.java:143-147) rejects a
            // zero cost outright, and requires the player's level to cover BOTH the lapis
            // cost (button index + 1) and the button's own level requirement.
            if level_req <= 0 {
                return false;
            }
            if (player.experience_level() < level_req || player.experience_level() < id + 1)
                && !player.is_creative()
            {
                return false;
            }

            let mut lapis_stack = self.inventory.get_stack(1).await;
            let lapis_cost = (id + 1) as u8;

            if !player.is_creative()
                && (lapis_stack.is_empty()
                    || !is_lapis(&lapis_stack)
                    || lapis_stack.item_count < lapis_cost)
            {
                return false;
            }

            // Perform enchantment
            let mut item_stack = self.inventory.get_stack(0).await;

            if item_stack.is_empty() || item_stack.has_enchantments() {
                return false;
            }

            let mut random = self.create_enchantment_random(id as usize);
            let mut enchantments =
                Self::get_enchantment_list(&mut random, &item_stack, id as usize, level_req);

            if enchantments.is_empty() {
                return false;
            }

            if player
                .fire_enchant_item_event(&item_stack, id, level_req, &mut enchantments)
                .await
                || enchantments.is_empty()
            {
                return false;
            }

            if !player.is_creative() {
                player.add_experience_levels(-(id + 1)).await;
                lapis_stack.decrement(lapis_cost);
                self.inventory.set_stack(1, lapis_stack).await;
            }

            // `EnchantmentMenu.clickMenuButton` (EnchantmentMenu.java:154-158): a plain book
            // is transmuted into an enchanted book before the levels are written.
            if item_stack.item == &Item::BOOK {
                item_stack.item = &Item::ENCHANTED_BOOK;
            }
            for (enchant, level) in enchantments {
                add_enchantment_for(&mut item_stack, enchant, level);
            }
            self.inventory.set_stack(0, item_stack).await;

            // Update seed
            player.set_enchantment_seed(rand::random()).await;
            self.enchantment_seed = player.enchantment_seed();

            self.update_enchantments(player).await;
            self.send_content_updates().await;

            player
                .increment_stat(
                    StatisticCategory::Custom,
                    CustomStatistic::EnchantItem as i32,
                    1,
                )
                .await;

            true
        })
    }

    fn quick_move<'a>(
        &'a mut self,
        player: &'a dyn InventoryPlayer, // FIX: Changed _player to player
        slot_index: i32,
    ) -> ItemStackFuture<'a> {
        Box::pin(async move {
            let mut stack_left = ItemStack::EMPTY.clone();
            let slot = self.get_behaviour().slots[slot_index as usize].clone();

            if slot.has_stack().await {
                let mut slot_stack = slot.get_stack().await;
                stack_left = slot_stack.clone();

                if slot_index < 2 {
                    // From enchanting to player
                    if !self
                        .insert_item(
                            &mut slot_stack,
                            2,
                            self.get_behaviour().slots.len() as i32,
                            true,
                        )
                        .await
                    {
                        return ItemStack::EMPTY.clone();
                    }
                } else {
                    // From player to enchanting
                    // Lapis check
                    if slot_stack.item == &Item::LAPIS_LAZULI {
                        if !self.insert_item(&mut slot_stack, 1, 2, false).await {
                            return ItemStack::EMPTY.clone();
                        }
                    } else if !self.insert_item(&mut slot_stack, 0, 1, false).await {
                        return ItemStack::EMPTY.clone();
                    }
                }

                if slot_stack.is_empty() {
                    slot.set_stack(ItemStack::EMPTY.clone()).await;
                } else {
                    slot.set_stack(slot_stack).await;
                }

                // CRITICAL FIX: Ensure the client is notified when shift-clicking items into the slots
                self.update_enchantments(player).await;
            }

            stack_left
        })
    }

    fn on_slot_click<'a>(
        &'a mut self,
        slot_index: i32,
        button: i32,
        action_type: pumpkin_protocol::java::server::play::SlotActionType,
        player: &'a dyn InventoryPlayer,
    ) -> ScreenHandlerFuture<'a, ()> {
        Box::pin(async move {
            self.internal_on_slot_click(slot_index, button, action_type, player)
                .await;
            if slot_index == 0 || slot_index == 1 {
                self.update_enchantments(player).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lapis_slot_only_accepts_lapis_lazuli() {
        assert!(is_lapis(&ItemStack::new(1, &Item::LAPIS_LAZULI)));
        assert!(!is_lapis(&ItemStack::new(1, &Item::DIRT)));
    }

    /// `Enchantment.isPrimaryItem` (`Enchantment.java:130-131`) is what the table rolls
    /// against; `supported_items` alone would let a table offer Thorns on boots/helmets, which
    /// vanilla never does (`thorns.json`: primary `enchantable/chest_armor`, supported
    /// `enchantable/armor`).
    #[test]
    fn table_rolls_thorns_only_on_chest_armor() {
        assert!(Enchantment::THORNS.can_enchant(&Item::DIAMOND_BOOTS));
        assert!(!Enchantment::THORNS.is_primary_item(&Item::DIAMOND_BOOTS));
        assert!(!Enchantment::THORNS.is_primary_item(&Item::DIAMOND_HELMET));
        assert!(Enchantment::THORNS.is_primary_item(&Item::DIAMOND_CHESTPLATE));
    }

    /// Fire Aspect supports every `enchantable/fire_aspect` item (maces and spears included)
    /// but is only ever *offered* on `enchantable/melee_weapon` (`fire_aspect.json`).
    #[test]
    fn table_rolls_fire_aspect_only_on_melee_weapons() {
        assert!(Enchantment::FIRE_ASPECT.is_primary_item(&Item::DIAMOND_SWORD));
        assert!(Enchantment::FIRE_ASPECT.can_enchant(&Item::MACE));
        assert!(!Enchantment::FIRE_ASPECT.is_primary_item(&Item::MACE));
    }

    /// An enchantment with no `primary_items` set must keep behaving exactly like before.
    #[test]
    fn primary_item_falls_back_to_supported_items() {
        assert!(Enchantment::UNBREAKING.primary_items.is_none());
        assert_eq!(
            Enchantment::UNBREAKING.can_enchant(&Item::DIAMOND_PICKAXE),
            Enchantment::UNBREAKING.is_primary_item(&Item::DIAMOND_PICKAXE)
        );
    }
}
