use crate::block::entities::BlockEntity;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{
    ContainerImpl, ContainerLootImpl, CustomNameImpl, DataComponentImpl, FireworkExplosionImpl,
    FireworkExplosionShape, FireworksImpl, ItemNameImpl, StoredEnchantmentsImpl,
    WrittenBookContentImpl,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag;
use pumpkin_data::{Block, BlockState, item::Item};
use std::sync::Arc;

use crate::block::entities::decorated_pot::DecoratedPotBlockEntity;
use pumpkin_util::{
    loot_table::{
        LootCondition, LootFireworkExplosionOperation, LootFunction, LootFunctionBonusParameter,
        LootFunctionNumberProvider, LootFunctionTypes, LootNameTarget, LootPoolEntry,
        LootPoolEntryTypes, LootTable,
    },
    random::{RandomGenerator, RandomImpl, get_seed, xoroshiro128::Xoroshiro},
    text::TextComponent,
};
use rand::RngExt;

#[derive(Default, Clone)]
pub struct LootContextParameters {
    pub explosion_radius: Option<f32>,
    pub block_state: Option<&'static BlockState>,
    pub killed_by_player: Option<bool>,
    pub luck: f32,
    pub this_entity: Option<&'static EntityType>,
    pub killer_entity: Option<&'static EntityType>,
    pub direct_killer_entity: Option<&'static EntityType>,
    pub position: Option<pumpkin_util::math::vector3::Vector3<f64>>,
    pub world_time: u64,
    pub damage_type: Option<DamageType>,
    pub tool: Option<ItemStack>,
    pub is_raining: Option<bool>,
    pub is_thundering: Option<bool>,
    /// Whether the killed entity was on fire at death time.
    /// Computed from `Entity.fire_ticks > 0`.
    pub is_on_fire: Option<bool>,
    /// Block entity captured before removal, read by block loot functions such as
    /// `ShulkerBoxBlock.getDrops` (`ShulkerBoxBlock.java:127-139`) and
    /// `DecoratedPotBlock.getDrops` (`DecoratedPotBlock.java:181-191`).
    pub block_entity: Option<Arc<dyn BlockEntity>>,
}

fn container_from_block_entity(entity: &dyn BlockEntity) -> Option<ContainerImpl> {
    // `ShulkerBoxBlock.getDrops` (`ShulkerBoxBlock.java:129-135`) copies the block entity's
    // dynamic contents into the replacement shulker box item.
    let nbt = entity.chunk_data_nbt()?;
    let items = nbt
        .get_list("Items")?
        .iter()
        .filter_map(|tag| {
            let item = tag.extract_compound()?;
            let slot = item.get_byte("Slot")? as u8;
            Some((slot, ItemStack::read_item_stack(item)?))
        })
        .collect();
    Some(ContainerImpl { items })
}

/// Vanilla `LootTable.createStackSplitter` (`LootTable.java:66-82`): a roll whose count is at
/// or above the item's max stack size is emitted as several stacks of at most `max_stack_size`
/// rather than one oversized stack.
///
/// The `isItemEnabled` feature-flag half of the vanilla lambda has no analogue here and is
/// deliberately not modelled.
fn push_split_stack(stacks: &mut Vec<ItemStack>, stack: ItemStack) {
    let max = stack.get_max_stack_size().max(1);
    if stack.item_count < max {
        stacks.push(stack);
        return;
    }

    let mut remaining = stack.item_count;
    while remaining > 0 {
        let count = max.min(remaining);
        stacks.push(stack.copy_with_count(count));
        remaining -= count;
    }
}

/// Resolves an item tag's members. Shared by `TagEntry.createItemStack` (`TagEntry.java:46-48`,
/// `expand: false`) below and by the pool-level `expand: true` fan-out
/// (`TagEntry.expandTag`, `TagEntry.java:50-65`) in `LootTableExt::get_loot`.
fn resolve_tag_items(name: &str) -> Vec<&'static Item> {
    // `get_tag_values` keys its map by the full namespaced id (e.g. `"minecraft:wool"`,
    // `crates/pumpkin-data/src/generated/tag.rs`), unlike `Item::from_registry_key` below,
    // which wants the bare path with no namespace.
    pumpkin_data::tag::get_tag_values(tag::RegistryKey::Item, name)
        .unwrap_or_default()
        .iter()
        .filter_map(|registry_key| {
            let item_key = registry_key
                .strip_prefix("minecraft:")
                .unwrap_or(registry_key);
            Item::from_registry_key(item_key)
        })
        .collect()
}

pub trait LootTableExt {
    fn get_loot(&self, params: LootContextParameters) -> Vec<ItemStack>;
}

impl LootTableExt for LootTable {
    fn get_loot(&self, params: LootContextParameters) -> Vec<ItemStack> {
        let mut stacks = Vec::new();
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));

        if let Some(pools) = self.pools {
            for pool in pools {
                if let Some(conditions) = pool.conditions
                    && !conditions.iter().all(|cond| cond.is_fulfilled(&params))
                {
                    continue;
                }

                let rolls = pool.rolls.get(&mut random) as i32
                    + (pool.bonus_rolls.get(&mut random) * params.luck).floor() as i32;

                for _ in 0..rolls {
                    let mut total_weight = 0;
                    // `Option<&Item>` fans an `expand: true` tag entry out into one candidate
                    // per tag item, each carrying the entry's own weight (`LootPool.addRandomItem`
                    // calling `LootPoolEntryContainer.expand`, `LootPool.java:70-77`, driven by
                    // `TagEntry.expandTag`, `TagEntry.java:50-65`), rather than contributing the
                    // entry's weight once and choosing an item internally.
                    let mut valid_entries: Vec<(&LootPoolEntry, i32, Option<&Item>)> = Vec::new();

                    for entry in pool.entries {
                        if entry
                            .conditions
                            .as_ref()
                            .is_none_or(|c| c.iter().all(|cond| cond.is_fulfilled(&params)))
                        {
                            let weight = (entry.weight as f32 + entry.quality as f32 * params.luck)
                                .floor() as i32;
                            let weight = weight.max(0);

                            if let LootPoolEntryTypes::Tag(tag) = &entry.content
                                && tag.expand
                            {
                                for item in resolve_tag_items(tag.name) {
                                    total_weight += weight;
                                    valid_entries.push((entry, weight, Some(item)));
                                }
                            } else {
                                total_weight += weight;
                                valid_entries.push((entry, weight, None));
                            }
                        }
                    }

                    if total_weight == 0 || valid_entries.is_empty() {
                        continue;
                    }

                    let mut r = random.next_bounded_i32(total_weight);

                    for (entry, weight, forced_item) in valid_entries {
                        r -= weight;
                        if r < 0 {
                            let loot = forced_item.map_or_else(
                                || entry.get_loot(&params),
                                |item| {
                                    let mut item_stacks = vec![ItemStack::new(1, item)];
                                    if let Some(functions) = entry.functions {
                                        for function in functions {
                                            function.apply(&mut item_stacks, &params);
                                        }
                                    }
                                    Some(item_stacks)
                                },
                            );
                            if let Some(mut loot) = loot {
                                // Vanilla decorates each selected entry with the pool's functions
                                // before the result consumer receives it (`LootPool.java:97-103`).
                                if let Some(functions) = pool.functions {
                                    for function in functions {
                                        function.apply(&mut loot, &params);
                                    }
                                }
                                for stack in loot {
                                    if stack.item_count > 0 {
                                        push_split_stack(&mut stacks, stack);
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
        stacks
    }
}

trait LootPoolEntryExt {
    fn get_loot(&self, params: &LootContextParameters) -> Option<Vec<ItemStack>>;
}

trait LootFunctionExt {
    fn apply(&self, stacks: &mut Vec<ItemStack>, params: &LootContextParameters);
}

fn apply_bonus(
    stacks: &mut [ItemStack],
    enchantment_name: &str,
    formula: &str,
    parameters: Option<&LootFunctionBonusParameter>,
    params: &LootContextParameters,
) {
    let enchantment_level = params.tool.as_ref().map_or(0, |tool| {
        pumpkin_data::Enchantment::from_name(enchantment_name)
            .map_or(0, |enchantment| tool.get_enchantment_level(enchantment))
    });
    if enchantment_level > 0 {
        for stack in stacks {
            match formula {
                "minecraft:binomial_with_bonus_count" => {
                    if let Some(LootFunctionBonusParameter::Probability { extra, probability }) =
                        parameters
                    {
                        let n = enchantment_level + *extra;
                        let mut extra_items = 0;
                        for _ in 0..n {
                            if rand::rng().random::<f32>() < *probability {
                                extra_items += 1;
                            }
                        }
                        stack.item_count = stack.item_count.saturating_add(extra_items as u8);
                    }
                }
                "minecraft:uniform_bonus_count" => {
                    if let Some(LootFunctionBonusParameter::Multiplier { bonus_multiplier }) =
                        parameters
                    {
                        let extra =
                            rand::rng().random_range(0..=(enchantment_level * *bonus_multiplier));
                        stack.item_count = stack.item_count.saturating_add(extra as u8);
                    }
                }
                "minecraft:ore_drops" if enchantment_level > 0 => {
                    let multiplier = rand::rng().random_range(0..=(enchantment_level + 1));
                    if multiplier > 0 {
                        stack.item_count = stack.item_count.saturating_mul(multiplier as u8);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Implements `SetBookCoverFunction.run` and `apply` from
/// `net/minecraft/world/level/storage/loot/functions/SetBookCoverFunction.java:42-54`.
/// The raw title is retained because Pumpkin has no server-side text-filtering service;
/// pages and the component's other fields remain unchanged when their loot fields are absent.
fn apply_set_book_cover(
    stack: &mut ItemStack,
    title: Option<&str>,
    author: Option<&str>,
    generation: Option<i32>,
) {
    let original = stack
        .get_data_component::<WrittenBookContentImpl>()
        .cloned()
        .unwrap_or_else(|| WrittenBookContentImpl {
            title: String::new(),
            author: String::new(),
            pages: Vec::new(),
            generation: 0,
        });
    let updated = WrittenBookContentImpl {
        title: title.map_or(original.title.clone(), str::to_owned),
        author: author.map_or(original.author.clone(), str::to_owned),
        pages: original.pages,
        generation: generation.unwrap_or(original.generation),
    };

    if let Some(content) = stack.get_data_component_mut::<WrittenBookContentImpl>() {
        *content = updated;
    } else {
        stack
            .patch
            .push((DataComponent::WrittenBookContent, Some(updated.to_dyn())));
    }
}

/// Implements `SetFireworkExplosion.run` and `apply` from
/// `net/minecraft/world/level/storage/loot/functions/SetFireworkExplosionFunction.java:53-65`.
/// The vanilla default is the small-ball explosion with empty colors and both flags false
/// (`SetFireworkExplosionFunction.java:29`), matching the component representation here.
fn apply_set_firework_explosion(
    stack: &mut ItemStack,
    shape: Option<&str>,
    colors: Option<&[i32]>,
    fade_colors: Option<&[i32]>,
    trail: Option<bool>,
    twinkle: Option<bool>,
) {
    let original = stack
        .get_data_component::<FireworkExplosionImpl>()
        .cloned()
        .unwrap_or_else(|| {
            FireworkExplosionImpl::new(
                FireworkExplosionShape::SmallBall,
                Vec::new(),
                Vec::new(),
                false,
                false,
            )
        });
    let updated = FireworkExplosionImpl::new(
        shape
            .and_then(FireworkExplosionShape::from_name)
            .unwrap_or(original.shape),
        colors.map_or(original.colors.clone(), <[i32]>::to_vec),
        fade_colors.map_or(original.fade_colors.clone(), <[i32]>::to_vec),
        trail.unwrap_or(original.has_trail),
        twinkle.unwrap_or(original.has_twinkle),
    );

    if let Some(explosion) = stack.get_data_component_mut::<FireworkExplosionImpl>() {
        *explosion = updated;
    } else {
        stack
            .patch
            .push((DataComponent::FireworkExplosion, Some(updated.to_dyn())));
    }
}

/// Implements `SetEnchantmentsFunction.run` from
/// `net/minecraft/world/level/storage/loot/functions/SetEnchantmentsFunction.java:53-75`.
/// Books are transmuted to enchanted books (`SetEnchantmentsFunction.java:54-56`), which use
/// stored enchantments; all other items use the ordinary enchantments component. Levels are
/// rounded by `NumberProvider.getInt` (`NumberProvider.java:7-12`) and clamped to 0..255 as in
/// `SetEnchantmentsFunction.java:61-72`.
fn apply_set_enchantments(
    stack: &mut ItemStack,
    enchantments: &[(&str, LootFunctionNumberProvider)],
    add: bool,
) {
    if stack.item.id == Item::BOOK.id {
        // Vanilla `SetEnchantmentsFunction.run` uses `ItemStack.transmuteCopy` here
        // (`SetEnchantmentsFunction.java:53-56`; `ItemStack.java:599-608`).
        *stack = stack.transmute_copy(&Item::ENCHANTED_BOOK);
    }

    let stored = stack.item.id == Item::ENCHANTED_BOOK.id;
    for (name, provider) in enchantments {
        let Some(enchantment) = pumpkin_data::Enchantment::from_name(name) else {
            continue;
        };
        let level = provider.generate().round().clamp(0.0, 255.0) as i32;

        if stored {
            if let Some(data) = stack.get_data_component_mut::<StoredEnchantmentsImpl>() {
                set_enchantment_level(&mut data.enchantment, enchantment, level, add);
            } else {
                stack.patch.push((
                    DataComponent::StoredEnchantments,
                    Some(
                        StoredEnchantmentsImpl {
                            enchantment: std::borrow::Cow::Owned(vec![(enchantment, level)]),
                        }
                        .to_dyn(),
                    ),
                ));
            }
        } else if let Some(data) =
            stack.get_data_component_mut::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
        {
            set_enchantment_level(&mut data.enchantment, enchantment, level, add);
        } else {
            stack.patch.push((
                DataComponent::Enchantments,
                Some(
                    pumpkin_data::data_component_impl::EnchantmentsImpl {
                        enchantment: std::borrow::Cow::Owned(vec![(enchantment, level)]),
                    }
                    .to_dyn(),
                ),
            ));
        }
    }
}

fn set_enchantment_level(
    levels: &mut std::borrow::Cow<'static, [(&'static pumpkin_data::Enchantment, i32)]>,
    enchantment: &'static pumpkin_data::Enchantment,
    level: i32,
    add: bool,
) {
    if let Some((_, current)) = levels
        .to_mut()
        .iter_mut()
        .find(|(existing, _)| *existing == enchantment)
    {
        *current = if add {
            (*current + level).clamp(0, 255)
        } else {
            level
        };
    } else {
        levels.to_mut().push((enchantment, level));
    }
}

/// Implements `SetFireworksFunction.run` and `apply` from
/// `net/minecraft/world/level/storage/loot/functions/SetFireworksFunction.java:39-49`.
/// Its default component is `new Fireworks(0, List.of())` from line 26; list behavior follows
/// `ListOperation.java:35-39,55-62,77-91,108-110,132-153`.
fn apply_set_fireworks(
    stack: &mut ItemStack,
    explosions: Option<&LootFireworkExplosionOperation>,
    flight_duration: Option<u8>,
) {
    let original = stack
        .get_data_component::<FireworksImpl>()
        .cloned()
        .unwrap_or_else(|| FireworksImpl::new(0, Vec::new()));
    let explosions = explosions.map_or_else(
        || original.explosions.clone(),
        |operation| {
            let values = operation
                .values
                .iter()
                .map(|value| {
                    FireworkExplosionImpl::new(
                        FireworkExplosionShape::from_name(value.shape)
                            .unwrap_or(FireworkExplosionShape::SmallBall),
                        value.colors.to_vec(),
                        value.fade_colors.to_vec(),
                        value.has_trail,
                        value.has_twinkle,
                    )
                })
                .collect::<Vec<_>>();
            operation
                .operation
                .apply(&original.explosions, &values, 256)
        },
    );
    let updated = FireworksImpl::new(
        flight_duration.map_or(original.flight_duration, i32::from),
        explosions,
    );

    if let Some(fireworks) = stack.get_data_component_mut::<FireworksImpl>() {
        *fireworks = updated;
    } else {
        stack
            .patch
            .push((DataComponent::Fireworks, Some(updated.to_dyn())));
    }
}

/// Implements `SetNameFunction.run` plus the `Target.component()` mapping from
/// `net/minecraft/world/level/storage/loot/functions/SetNameFunction.java:91-94,106-128`.
/// The name arrives exactly as codegen captured it: text-component JSON or a plain
/// string. Vanilla's optional entity-resolution half (`createResolver`,
/// `SetNameFunction.java:69-88`) needs a command source stack the headless loot
/// context does not carry, and no shipped loot table passes `entity`.
fn apply_set_name(stack: &mut ItemStack, raw_name: &str, target: LootNameTarget) {
    match target {
        LootNameTarget::CustomName => {
            let name = serde_json::from_str::<TextComponent>(raw_name)
                .unwrap_or_else(|_| TextComponent::text(raw_name.to_string()));
            if let Some(component) = stack.get_data_component_mut::<CustomNameImpl>() {
                component.name = name;
            } else {
                stack.patch.push((
                    DataComponent::CustomName,
                    Some(Box::new(CustomNameImpl { name }).to_dyn()),
                ));
            }
        }
        LootNameTarget::ItemName => {
            // Mirrors what `ItemNameImpl.read_data` accepts: a translate key, a literal
            // text key, or a bare string (see `ItemNameImpl` in
            // `crates/pumpkin-data/src/data_component_impl/basic.rs`).
            let name = match serde_json::from_str::<serde_json::Value>(raw_name) {
                Ok(serde_json::Value::Object(map)) => map
                    .get("translate")
                    .or_else(|| map.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| raw_name.to_string(), ToString::to_string),
                Ok(serde_json::Value::String(value)) => value,
                _ => raw_name.to_string(),
            };
            let name = std::borrow::Cow::Owned(name);
            if let Some(component) = stack.get_data_component_mut::<ItemNameImpl>() {
                component.name = name;
            } else {
                stack.patch.push((
                    DataComponent::ItemName,
                    Some(Box::new(ItemNameImpl { name }).to_dyn()),
                ));
            }
        }
    }
}

impl LootFunctionExt for LootFunction {
    #[allow(clippy::too_many_lines)]
    fn apply(&self, stacks: &mut Vec<ItemStack>, params: &LootContextParameters) {
        if let Some(conditions) = self.conditions
            && !conditions.iter().all(|cond| cond.is_fulfilled(params))
        {
            return;
        }

        match &self.content {
            // `DiscardItem.run` (`DiscardItem.java:23-25`) returns `ItemStack.EMPTY`; clearing
            // the vector is the equivalent for this evaluator's multi-stack representation.
            LootFunctionTypes::DiscardItem => stacks.clear(),
            LootFunctionTypes::SetCount { count, add } => {
                for stack in stacks {
                    if *add {
                        stack.item_count += count.generate().round() as u8;
                    } else {
                        stack.item_count = count.generate().round() as u8;
                    }
                }
            }
            // `SetItemDamageFunction.run` (`SetItemDamageFunction.java:46-56`) computes a
            // remaining-durability fraction, then floors it into the item's damage component.
            LootFunctionTypes::SetItemDamage { damage, add } => {
                for stack in stacks {
                    let Some(max_damage) = stack.get_max_damage() else {
                        tracing::warn!("Couldn't set damage of loot item");
                        continue;
                    };
                    let base = if *add {
                        1.0 - stack.get_damage() as f32 / max_damage as f32
                    } else {
                        0.0
                    };
                    let remaining = 1.0 - (damage.generate() + base).clamp(0.0, 1.0);
                    stack.set_damage((remaining * max_damage as f32).floor() as i32);
                }
            }
            // `SetContainerLootTable.run` (`SetContainerLootTable.java:50-57`) stores the
            // table key and seed and leaves empty stacks unchanged.
            LootFunctionTypes::SetContainerLootTable { name, seed } => {
                for stack in stacks {
                    if stack.is_empty() {
                        continue;
                    }
                    if let Some(loot) = stack.get_data_component_mut::<ContainerLootImpl>() {
                        (*name).clone_into(&mut loot.loot_table);
                        loot.seed = *seed;
                    } else {
                        stack.patch.push((
                            DataComponent::ContainerLoot,
                            Some(Box::new(ContainerLootImpl {
                                loot_table: (*name).to_owned(),
                                seed: *seed,
                            })),
                        ));
                    }
                }
            }
            LootFunctionTypes::SetEnchantments { enchantments, add } => {
                for stack in stacks {
                    apply_set_enchantments(stack, enchantments, *add);
                }
            }
            LootFunctionTypes::LimitCount { min, max } => {
                if let Some(min) = min.map(|min| min.round() as u8) {
                    for stack in stacks.iter_mut() {
                        if stack.item_count < min {
                            stack.item_count = min;
                        }
                    }
                }

                if let Some(max) = max.map(|max| max.round() as u8) {
                    for stack in stacks.iter_mut() {
                        if stack.item_count > max {
                            stack.item_count = max;
                        }
                    }
                }
            }
            LootFunctionTypes::ExplosionDecay => {
                if let Some(radius) = params.explosion_radius {
                    let survival_chance = 1.0 / radius;
                    for stack in stacks.iter_mut() {
                        let mut survived = 0;
                        for _ in 0..stack.item_count {
                            if rand::rng().random::<f32>() <= survival_chance {
                                survived += 1;
                            }
                        }
                        stack.item_count = survived;
                    }
                    // Remove empty stacks
                    stacks.retain(|stack| stack.item_count > 0);
                }
            }
            LootFunctionTypes::ApplyBonus {
                enchantment,
                formula,
                parameters,
            } => {
                apply_bonus(stacks, enchantment, formula, parameters.as_ref(), params);
            }
            LootFunctionTypes::EnchantedCountIncrease {
                enchantment,
                count,
                limit,
            } => {
                let level = params.tool.as_ref().map_or(0.0, |tool| {
                    pumpkin_data::Enchantment::from_name(enchantment)
                        .map_or(0.0, |enc| tool.get_enchantment_level(enc) as f32)
                });
                let mut additional = (count.generate() * level).round() as u32;
                if let Some(lim) = limit {
                    let lim_u32 = lim.round() as u32;
                    if additional > lim_u32 {
                        additional = lim_u32;
                    }
                }
                for stack in stacks {
                    stack.item_count = stack.item_count.saturating_add(additional as u8);
                }
            }
            LootFunctionTypes::CopyComponents { source, include } => {
                if *source != "block_entity" {
                    tracing::warn!(
                        "CopyComponents not supported from source: {} for {:?}",
                        source,
                        include
                    );
                } else if include.contains(&"minecraft:container") {
                    // `ShulkerBoxBlock.getDrops` (`ShulkerBoxBlock.java:127-139`) preserves
                    // the stored inventory in the dropped shulker box.
                    if let Some(block_entity) = params.block_entity.as_deref()
                        && let Some(container) = container_from_block_entity(block_entity)
                    {
                        for stack in stacks {
                            if let Some(existing) = stack.get_data_component_mut::<ContainerImpl>()
                            {
                                existing.items.clone_from(&container.items);
                            } else {
                                stack.patch.push((
                                    DataComponent::Container,
                                    Some(Box::new(container.clone()).to_dyn()),
                                ));
                            }
                        }
                    }
                } else if include.contains(&"minecraft:pot_decorations") {
                    // `DecoratedPotBlock.getDrops` (`DecoratedPotBlock.java:181-191`) copies
                    // `pot_decorations` from the block entity into the uncracked pot item.
                    if let Some(block_entity) = params.block_entity.as_ref()
                        && let Some(pot) = block_entity
                            .as_any()
                            .downcast_ref::<DecoratedPotBlockEntity>()
                        && let Some(decorations) = pot.decorations()
                    {
                        for stack in stacks {
                            stack.patch.push((
                                DataComponent::PotDecorations,
                                Some(
                                    Box::new(
                                        pumpkin_data::data_component_impl::PotDecorationsImpl {
                                            decorations: decorations.clone(),
                                        },
                                    )
                                    .to_dyn(),
                                ),
                            ));
                        }
                    }
                }
            }
            LootFunctionTypes::CopyState {
                block: _,
                properties,
            } => {
                if let Some(state) = params.block_state
                    && let Some(props_data) =
                        Block::properties(Block::from_state_id(state.id), state.id)
                {
                    let actual_props = props_data.to_props();
                    let mut properties_to_copy = std::collections::BTreeMap::new();
                    for &prop_name in *properties {
                        if let Some((_, value)) = actual_props.iter().find(|(k, _)| k == &prop_name)
                        {
                            properties_to_copy.insert(prop_name.to_string(), value.to_string());
                        }
                    }
                    if !properties_to_copy.is_empty() {
                        for stack in stacks.iter_mut() {
                            if let Some(block_state_comp) = stack.get_data_component_mut::<pumpkin_data::data_component_impl::BlockStateImpl>() {
                                    let mut props = block_state_comp.properties.to_mut().clone();
                                    for (k, v) in &properties_to_copy {
                                        if let Some(pos) = props.iter().position(|(pk, _)| pk.as_ref() == k) {
                                            props[pos].1 = std::borrow::Cow::Owned(v.clone());
                                        } else {
                                            props.push((std::borrow::Cow::Owned(k.clone()), std::borrow::Cow::Owned(v.clone())));
                                        }
                                    }
                                    block_state_comp.properties = std::borrow::Cow::Owned(props);
                                } else {
                                    let properties: Vec<(std::borrow::Cow<'static, str>, std::borrow::Cow<'static, str>)> = properties_to_copy
                                        .iter()
                                        .map(|(k, v)| (std::borrow::Cow::Owned(k.clone()), std::borrow::Cow::Owned(v.clone())))
                                        .collect();
                                    stack.patch.push((
                                        pumpkin_data::data_component::DataComponent::BlockState,
                                        Some(Box::new(pumpkin_data::data_component_impl::BlockStateImpl {
                                            properties: std::borrow::Cow::Owned(properties),
                                        })),
                                    ));
                                }
                        }
                    }
                }
            }
            LootFunctionTypes::SetOminousBottleAmplifier => {
                let amplifier = rand::random_range(0..5); // Random 0 to 4
                for stack in stacks.iter_mut() {
                    if let Some(amplifier_comp) = stack.get_data_component_mut::<pumpkin_data::data_component_impl::OminousBottleAmplifierImpl>() {
                        amplifier_comp.amplifier = amplifier;
                    } else {
                        stack.patch.push((
                            pumpkin_data::data_component::DataComponent::OminousBottleAmplifier,
                            Some(Box::new(pumpkin_data::data_component_impl::OminousBottleAmplifierImpl {
                                amplifier,
                            })),
                        ));
                    }
                }
            }
            LootFunctionTypes::SetPotion { id } => {
                let name = id.strip_prefix("minecraft:").unwrap_or(id);
                if let Some(potion) = pumpkin_data::potion::Potion::from_name(name) {
                    let potion_id = Some(potion.id as i32);
                    for stack in stacks.iter_mut() {
                        if let Some(potion_contents) = stack.get_data_component_mut::<pumpkin_data::data_component_impl::PotionContentsImpl>() {
                            potion_contents.potion_id = potion_id;
                        } else {
                            stack.patch.push((
                                pumpkin_data::data_component::DataComponent::PotionContents,
                                Some(Box::new(pumpkin_data::data_component_impl::PotionContentsImpl {
                                    potion_id,
                                    custom_color: None,
                                    custom_effects: Vec::new(),
                                    custom_name: None,
                                })),
                            ));
                        }
                    }
                }
            }
            LootFunctionTypes::FurnaceSmelt => {
                for stack in stacks.iter_mut() {
                    for recipe_type in pumpkin_data::recipes::RECIPES_COOKING {
                        if let pumpkin_data::recipes::CookingRecipeType::Smelting(recipe) =
                            recipe_type
                            && recipe.ingredient.match_item(stack.item)
                        {
                            let result_key = recipe
                                .result
                                .id
                                .strip_prefix("minecraft:")
                                .unwrap_or(recipe.result.id);
                            if let Some(smelted_item) = Item::from_registry_key(result_key) {
                                stack.item = smelted_item;
                            }
                            break;
                        }
                    }
                }
            }
            LootFunctionTypes::SetBookCover {
                title,
                author,
                generation,
            } => {
                for stack in stacks {
                    apply_set_book_cover(stack, *title, *author, *generation);
                }
            }
            LootFunctionTypes::SetFireworkExplosion {
                shape,
                colors,
                fade_colors,
                trail,
                twinkle,
            } => {
                for stack in stacks {
                    apply_set_firework_explosion(
                        stack,
                        *shape,
                        *colors,
                        *fade_colors,
                        *trail,
                        *twinkle,
                    );
                }
            }
            LootFunctionTypes::SetItem { item } => {
                if let Some(item) = Item::from_registry_key(item) {
                    for stack in stacks {
                        // Vanilla `SetItemFunction.run` returns `transmuteCopy` here
                        // (`SetItemFunction.java:28-31`; `ItemStack.java:599-608`).
                        *stack = stack.transmute_copy(item);
                    }
                }
            }
            LootFunctionTypes::SetFireworks {
                explosions,
                flight_duration,
            } => {
                for stack in stacks {
                    apply_set_fireworks(stack, explosions.as_ref(), *flight_duration);
                }
            }
            LootFunctionTypes::SetName { name, target } => {
                for stack in stacks {
                    apply_set_name(stack, name, *target);
                }
            }
            // `SetContainerContents.run`
            // (`net/minecraft/world/level/storage/loot/functions/SetContainerContents.java:47-56`):
            // expand every entry through the regular pool-entry machinery, split oversized
            // stacks with the table splitter (`LootTable.createStackSplitter`), and store
            // them slot-indexed in the container component; empty stacks are untouched
            // (:48-50).
            LootFunctionTypes::SetContainerContents { entries } => {
                for stack in stacks.iter_mut() {
                    if stack.is_empty() {
                        continue;
                    }
                    let mut contents: Vec<ItemStack> = Vec::new();
                    for entry in *entries {
                        if let Some(loot) = entry.get_loot(params) {
                            for item_stack in loot {
                                if item_stack.item_count > 0 {
                                    push_split_stack(&mut contents, item_stack);
                                }
                            }
                        }
                    }
                    let items: Vec<(u8, ItemStack)> = contents
                        .into_iter()
                        .enumerate()
                        .map(|(slot, item_stack)| (slot as u8, item_stack))
                        .collect();
                    if let Some(container) = stack.get_data_component_mut::<ContainerImpl>() {
                        container.items = items;
                    } else {
                        stack.patch.push((
                            DataComponent::Container,
                            Some(Box::new(ContainerImpl { items }).to_dyn()),
                        ));
                    }
                }
            }
        }
    }
}

impl LootPoolEntryExt for LootPoolEntry {
    fn get_loot(&self, params: &LootContextParameters) -> Option<Vec<ItemStack>> {
        if let Some(conditions) = self.conditions
            && !conditions.iter().all(|cond| cond.is_fulfilled(params))
        {
            return None;
        }

        let mut stacks = self.content.get_stacks(params);

        if let Some(functions) = self.functions {
            for function in functions {
                function.apply(&mut stacks, params);
            }
        }

        Some(stacks)
    }
}

trait LootPoolEntryTypesExt {
    fn get_stacks(&self, params: &LootContextParameters) -> Vec<ItemStack>;
}

impl LootPoolEntryTypesExt for LootPoolEntryTypes {
    fn get_stacks(&self, params: &LootContextParameters) -> Vec<ItemStack> {
        match self {
            // `DecoratedPotBlock.getDrops` (`DecoratedPotBlock.java:181-188`) supplies
            // the four stored decorations for the cracked-pot `sherds` dynamic drop.
            Self::Dynamic(entry) if entry.name == "minecraft:sherds" => params
                .block_entity
                .as_ref()
                .and_then(|block_entity| {
                    block_entity
                        .as_any()
                        .downcast_ref::<DecoratedPotBlockEntity>()
                })
                .and_then(DecoratedPotBlockEntity::decorations)
                .map_or_else(Vec::new, |decorations| {
                    decorations
                        .iter()
                        .filter_map(|decoration| {
                            Item::from_registry_key(
                                decoration.strip_prefix("minecraft:").unwrap_or(decoration),
                            )
                        })
                        .map(|item| ItemStack::new(1, item))
                        .collect()
                }),
            // An empty pool and an unhandled dynamic drop both yield nothing.
            Self::Empty | Self::Dynamic(_) => Vec::new(),
            Self::LootTable(entry) => {
                let key = entry
                    .value
                    .strip_prefix("minecraft:")
                    .unwrap_or(entry.value);
                // First try chest loot tables.
                pumpkin_data::chest_loot_table::get_chest_loot_table(&format!("minecraft:{key}"))
                    .map_or_else(Vec::new, |chest_table| {
                        // We don't have a seed here, but we can generate a random one.
                        let seed: i64 = rand::random();
                        generate_chest_loot(chest_table, seed)
                    })
            }
            Self::Item(item_entry) => {
                let key = item_entry
                    .name
                    .strip_prefix("minecraft:")
                    .unwrap_or(item_entry.name);
                Item::from_registry_key(key)
                    .map_or_else(Vec::new, |item| vec![ItemStack::new(1, item)])
            }
            Self::Tag(tag) => {
                let items = resolve_tag_items(tag.name);
                if items.is_empty() {
                    return Vec::new();
                }

                if tag.expand {
                    // Reached only when this entry sits somewhere `LootTableExt::get_loot`'s
                    // top-level pool-entry fan-out (`TagEntry.expandTag`, `TagEntry.java:50-65`)
                    // doesn't apply, e.g. nested inside an `Alternatives`/`Sequence`/`Group`
                    // entry. Vanilla still fans out to one weighted candidate per tag item in
                    // that case (`LootPoolEntryContainer.expand`); this uniform pick is an
                    // approximation, kept because no shipped loot table nests an `expand: true`
                    // tag this way to exercise it.
                    let index = rand::random_range(0..items.len() as i32) as usize;
                    vec![ItemStack::new(1, items[index])]
                } else {
                    // `TagEntry.createItemStack` (`TagEntry.java:46-48`): yield one stack of
                    // every item in the tag.
                    items
                        .into_iter()
                        .map(|item| ItemStack::new(1, item))
                        .collect()
                }
            }
            Self::Alternatives(alternative_entry) => {
                for entry in alternative_entry.children {
                    if let Some(loot) = entry.get_loot(params) {
                        return loot;
                    }
                }
                Vec::new()
            }
            Self::Sequence(sequence_entry) => {
                let mut stacks = Vec::new();
                for entry in sequence_entry.children {
                    if entry
                        .conditions
                        .as_ref()
                        .is_some_and(|c| !c.iter().all(|cond| cond.is_fulfilled(params)))
                    {
                        break;
                    }

                    match entry.get_loot(params) {
                        Some(loot) => stacks.extend(loot),
                        // get_loot returning None also signals failure — stop.
                        None => break,
                    }
                }
                stacks
            }

            Self::Group(group_entry) => {
                let mut stacks = Vec::new();
                for entry in group_entry.children {
                    if let Some(loot) = entry.get_loot(params) {
                        stacks.extend(loot);
                    }
                }
                stacks
            }
        }
    }
}

trait LootConditionExt {
    fn is_fulfilled(&self, params: &LootContextParameters) -> bool;
}

fn compare_entity_type(expected_type: &str, actual: &EntityType) -> bool {
    let expected = expected_type
        .strip_prefix("minecraft:")
        .unwrap_or(expected_type);
    let actual = actual
        .resource_name
        .strip_prefix("minecraft:")
        .unwrap_or(actual.resource_name);
    expected == actual
}

fn check_block_state_property(state: &BlockState, properties: &[(&str, &str)]) -> bool {
    let block_actual_properties = match Block::properties(Block::from_state_id(state.id), state.id)
    {
        Some(props_data) => props_data.to_props(), // Assuming to_props() returns HashMap<String, String>
        None => {
            return properties.is_empty();
        }
    };

    properties.iter().all(|(expected_key, expected_value)| {
        block_actual_properties
            .iter()
            .find(|(actual_key, _)| actual_key == expected_key)
            .is_some_and(|(_, actual_value_string)| actual_value_string == expected_value)
    })
}

fn check_damage_source_properties(
    params: &LootContextParameters,
    expected_source_type: Option<&str>,
    expected_direct_type: Option<&str>,
) -> bool {
    if params.damage_type.is_none() {
        return false;
    }
    if let Some(expected) = expected_source_type {
        if let Some(actual) = params.killer_entity {
            if !compare_entity_type(expected, actual) {
                return false;
            }
        } else {
            return false;
        }
    }
    if let Some(expected) = expected_direct_type {
        if let Some(actual) = params.direct_killer_entity {
            if !compare_entity_type(expected, actual) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

impl LootConditionExt for LootCondition {
    #[allow(clippy::too_many_lines)]
    fn is_fulfilled(&self, params: &LootContextParameters) -> bool {
        match self {
            Self::SurvivesExplosion => {
                if let Some(radius) = params.explosion_radius {
                    return rand::rng().random::<f32>() <= 1.0 / radius;
                }
                true
            }
            Self::RandomChance { chance } => rand::rng().random::<f32>() < *chance,
            Self::EntityProperties {
                entity,
                expected_type,
                is_on_fire,
                mainhand_enchantment_tag,
            } => {
                // Mirrors vanilla `EntityTarget` resolution from `LootContext.java:148-186`.
                let target = match *entity {
                    "this" => params.this_entity,
                    "attacker" | "killer" | "attacking_player" => params.killer_entity,
                    "direct_attacker" | "direct_killer" => params.direct_killer_entity,
                    _ => None,
                };
                if let Some(target) = target {
                    if let Some(expected) = expected_type
                        && !compare_entity_type(expected, target)
                    {
                        return false;
                    }
                    // Mirrors vanilla `EntityFlagsPredicate.isOnFire` check.
                    if let Some(expected_fire) = is_on_fire {
                        let actual_fire = params.is_on_fire.unwrap_or(false);
                        if actual_fire != *expected_fire {
                            return false;
                        }
                    }
                    // Mirrors vanilla enchantment tag lookup for smelts_loot.
                    if let Some(tag_name) = mainhand_enchantment_tag {
                        let tag = tag_name.strip_prefix('#').unwrap_or(tag_name);
                        let has_enchant = params.tool.as_ref().is_some_and(|tool| {
                            pumpkin_data::tag::get_tag_ids(
                                pumpkin_data::tag::RegistryKey::Enchantment,
                                tag,
                            )
                            .is_some_and(|tag_ids| {
                                tag_ids.iter().any(|&ench_id| {
                                    pumpkin_data::Enchantment::from_id(ench_id as u8)
                                        .is_some_and(|enc| tool.get_enchantment_level(enc) > 0)
                                })
                            })
                        });
                        if !has_enchant {
                            return false;
                        }
                    }
                    true
                } else {
                    false
                }
            }
            Self::KilledByPlayer => params.killed_by_player.unwrap_or(false),
            Self::BlockStateProperty {
                block: _,
                properties,
            } => {
                if let Some(state) = &params.block_state {
                    return check_block_state_property(state, properties);
                }
                false
            }
            Self::Inverted(term) => !term.is_fulfilled(params),
            Self::AnyOf(terms) => terms.iter().any(|cond| cond.is_fulfilled(params)),
            Self::AllOf(terms) => terms.iter().all(|cond| cond.is_fulfilled(params)),
            Self::RandomChanceWithEnchantedBonus {
                enchantment,
                chances,
            } => chances.as_ref().is_some_and(|chances| {
                let level = params.tool.as_ref().map_or(0, |tool| {
                    pumpkin_data::Enchantment::from_name(enchantment)
                        .map_or(0, |enc| tool.get_enchantment_level(enc) as usize)
                });
                let chance = chances.get(level).unwrap_or(chances.last().unwrap_or(&0.0));
                rand::rng().random::<f32>() < *chance
            }),
            Self::TableBonus {
                enchantment,
                chances,
            } => {
                let level = params.tool.as_ref().map_or(0, |tool| {
                    pumpkin_data::Enchantment::from_name(enchantment)
                        .map_or(0, |enc| tool.get_enchantment_level(enc) as usize)
                });
                let chance = chances.get(level).unwrap_or(chances.last().unwrap_or(&0.0));
                rand::rng().random::<f32>() < *chance
            }
            Self::TimeCheck { range, period } => {
                let mut time = params.world_time;
                if let Some(period) = period {
                    time %= period;
                }
                let (min, max) = range;
                let val = time as f32;
                min.is_none_or(|min| val >= min) && max.is_none_or(|max| val <= max)
            }
            Self::ValueCheck { value, range } => {
                let mut rng = Xoroshiro::from_seed(get_seed());
                let val = value.get(&mut rng);
                let (min, max) = range;
                min.is_none_or(|min| val >= min) && max.is_none_or(|max| val <= max)
            }
            Self::DamageSourceProperties {
                expected_source_type,
                expected_direct_type,
            } => {
                check_damage_source_properties(params, *expected_source_type, *expected_direct_type)
            }
            Self::WeatherCheck {
                raining,
                thundering,
            } => {
                let r_match = raining.is_none_or(|r| params.is_raining.unwrap_or(false) == r);
                let t_match = thundering.is_none_or(|t| params.is_thundering.unwrap_or(false) == t);
                r_match && t_match
            }
            Self::MatchTool { items } => params.tool.as_ref().is_some_and(|tool| {
                items.as_ref().map_or_else(
                    || {
                        pumpkin_data::Enchantment::from_name("minecraft:silk_touch")
                            .is_some_and(|silk_touch| tool.get_enchantment_level(silk_touch) > 0)
                    },
                    |items| {
                        items.iter().any(|&item_name| {
                            let expected =
                                item_name.strip_prefix("minecraft:").unwrap_or(item_name);
                            let actual = tool
                                .item
                                .registry_key
                                .strip_prefix("minecraft:")
                                .unwrap_or(tool.item.registry_key);
                            expected == actual
                        })
                    },
                )
            }),
            Self::LocationCheck { expected_biome, .. } => expected_biome.is_none(),
            Self::EntityScores { entity } => {
                tracing::warn!("EntityScores check not supported for entity: {}", entity);
                false
            }
            Self::Reference { name } => {
                tracing::warn!("Loot condition reference not supported: {}", name);
                false
            }
            Self::EnchantmentActiveCheck { active } => {
                params.tool.as_ref().map_or(!*active, |tool| {
                    let has_enchantments = tool
                        .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
                        .is_some_and(|e| !e.enchantment.is_empty());
                    has_enchantments == *active
                })
            }
        }
    }
}

trait LootFunctionNumberProviderExt {
    fn generate(&self) -> f32;
}

impl LootFunctionNumberProviderExt for LootFunctionNumberProvider {
    fn generate(&self) -> f32 {
        match self {
            Self::Constant { value } => *value,
            Self::Uniform { min, max } => rand::random::<f32>() * (max - min) + min,
            Self::Binomial { n, p } => (0..n.floor() as u32).fold(0.0, |c, _| {
                if rand::rng().random_bool(f64::from(*p)) {
                    c + 1.0
                } else {
                    c
                }
            }),
        }
    }
}

/// Vanilla `EnchantRandomlyFunction.run` + `enchantItem`
/// (`net/minecraft/world/level/storage/loot/functions/EnchantRandomlyFunction.java:71-102`):
/// resolves the function's candidate enchantments (a `#`-prefixed tag key expands
/// to its members; an empty list is vanilla's absent-`options` default of every
/// registered enchantment), drops candidates incompatible with the target item,
/// picks one uniformly (`Util.getRandomSafe`, :80) and rolls a level with
/// `Mth.nextInt(random, getMinLevel(), getMaxLevel())` (:91) - every enchantment's
/// minimum level is 1.
///
/// Books bypass the compatibility filter and are transmuted to enchanted books
/// carrying a stored-enchantments component (:73,92-94). The
/// `include_additional_cost_component` half of `enchantItem` (:97-99) only fires
/// for villager-trade loot contexts that allow the additional-cost parameter and
/// has no chest-loot analogue here.
fn apply_enchant_randomly(stack: &mut ItemStack, options: &[&'static str], rng: &mut Xoroshiro) {
    let mut candidates: Vec<&'static pumpkin_data::Enchantment> = options
        .iter()
        .flat_map(|option| {
            option.strip_prefix('#').map_or_else(
                || {
                    pumpkin_data::Enchantment::from_name(option)
                        .into_iter()
                        .collect::<Vec<_>>()
                },
                |tag_key| {
                    pumpkin_data::tag::get_tag_ids(
                        pumpkin_data::tag::RegistryKey::Enchantment,
                        tag_key,
                    )
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|id| pumpkin_data::Enchantment::from_id(u8::try_from(*id).ok()?))
                    .collect::<Vec<_>>()
                },
            )
        })
        .collect();

    // `shouldCheckCompatibility = !targetIsBook && this.onlyCompatible`
    // (:73-74); no shipped chest table disables `only_compatible`.
    if stack.item.id != Item::BOOK.id {
        let item = stack.item;
        candidates.retain(|enchantment| enchantment.can_enchant(item));
    }

    let Some(enchantment) = candidates.get(rng.next_bounded_i32(candidates.len() as i32) as usize)
    else {
        tracing::warn!(
            "Couldn't find a compatible enchantment for {}",
            stack.item.registry_key
        );
        return;
    };
    let level = rng.next_bounded_i32(enchantment.max_level) + 1;

    if stack.item.id == Item::BOOK.id {
        // Vanilla `itemStack.enchant` on a book writes the stored-enchantments
        // component of an enchanted book (:92-96).
        *stack = ItemStack::new_with_component(
            stack.item_count,
            &Item::ENCHANTED_BOOK,
            vec![(
                DataComponent::StoredEnchantments,
                Some(Box::new(StoredEnchantmentsImpl {
                    enchantment: std::borrow::Cow::Owned(vec![(*enchantment, level)]),
                }) as Box<dyn DataComponentImpl>),
            )],
        );
    } else {
        stack.enchant(enchantment, level);
    }
}

/// Generates a list of items from a `ChestLootTable` using a deterministic seed.
#[must_use]
pub fn generate_chest_loot(
    table: &pumpkin_util::chest_loot_table::ChestLootTable,
    seed: i64,
) -> Vec<ItemStack> {
    use pumpkin_util::random::RandomImpl;

    let mut rng = Xoroshiro::from_seed(seed as u64);
    let mut items_to_place: Vec<ItemStack> = Vec::new();

    for pool in table.pools {
        let range = pool.max_rolls - pool.min_rolls;
        let rolls = pool.min_rolls
            + if range > 0 {
                rng.next_bounded_i32(range + 1)
            } else {
                0
            };

        for _ in 0..rolls {
            let entry_weight: i32 = pool.entries.iter().map(|e| e.weight).sum();
            let total_weight = entry_weight + pool.empty_weight;
            if total_weight == 0 {
                continue;
            }

            let mut pick = rng.next_bounded_i32(total_weight);

            // Subtract empty weight first (if the pick lands here, it yields nothing).
            pick -= pool.empty_weight;
            if pick < 0 {
                continue;
            }

            for entry in pool.entries {
                pick -= entry.weight;
                if pick < 0 {
                    let count_range = entry.max_count - entry.min_count;
                    let count = entry.min_count
                        + if count_range > 0 {
                            rng.next_bounded_i32(count_range + 1)
                        } else {
                            0
                        };

                    // Strip "minecraft:" prefix because from_registry_key uses short keys.
                    let item_key = entry.item.strip_prefix("minecraft:").unwrap_or(entry.item);

                    if let Some(item) = Item::from_registry_key(item_key) {
                        let mut stack = ItemStack::new(count as u8, item);
                        if let Some(options) = entry.enchant_randomly {
                            apply_enchant_randomly(&mut stack, options, &mut rng);
                        }
                        items_to_place.push(stack);
                    }
                    break;
                }
            }
        }
    }

    items_to_place
}

/// Items are scattered randomly across the 27 chest slots.
pub async fn fill_chest_inventory(
    inventory: &std::sync::Arc<dyn pumpkin_world::inventory::Inventory>,
    table: &pumpkin_util::chest_loot_table::ChestLootTable,
    seed: i64,
) {
    let mut items_to_place = generate_chest_loot(table, seed);

    if items_to_place.is_empty() {
        return;
    }

    let inv_size = inventory.size(); // 27 for a normal chest
    let mut rng = Xoroshiro::from_seed(seed as u64);
    let free_slots = inv_size;

    // Split large stacks across extra slots then shuffle.
    shuffle_and_split_items(&mut items_to_place, free_slots, &mut rng);

    // Pick random distinct slots and place each item.
    let mut available_slots: Vec<usize> = (0..inv_size).collect();
    // Shuffle available slots using Fisher-Yates so item order from above maps to random slots.
    for i in (1..available_slots.len()).rev() {
        let j = rng.next_bounded_i32((i + 1) as i32) as usize;
        available_slots.swap(i, j);
    }

    for item in items_to_place {
        let Some(slot) = available_slots.pop() else {
            break;
        };
        inventory.set_stack(slot, item).await;
    }
}

/// Stacks with count > 1 are split at a random midpoint and redistributed while
/// there are more free slots than total items. Then everything is shuffled.
fn shuffle_and_split_items(
    result: &mut Vec<ItemStack>,
    available_slots: usize,
    rng: &mut Xoroshiro,
) {
    use pumpkin_util::random::RandomImpl;

    // Drain all items with count > 1 into a splittable list.
    let mut splittable: Vec<ItemStack> = Vec::new();
    let mut i = 0;
    while i < result.len() {
        if result[i].item_count > 1 {
            splittable.push(result.swap_remove(i));
        } else {
            i += 1;
        }
    }

    // While there are more free slots than total items, split a random stack.
    while available_slots > result.len() + splittable.len() && !splittable.is_empty() {
        let idx = rng.next_bounded_i32(splittable.len() as i32) as usize;
        let mut stack = splittable.swap_remove(idx);

        let count = stack.item_count as i32;
        // Split off [1, count/2] items.
        let split_off = 1 + rng.next_bounded_i32(count / 2);
        stack.item_count = (count - split_off) as u8;
        let mut copy = stack.clone();
        copy.item_count = split_off as u8;

        if stack.item_count > 1 {
            splittable.push(stack);
        } else {
            result.push(stack);
        }
        if copy.item_count > 1 {
            splittable.push(copy);
        } else {
            result.push(copy);
        }
    }

    // Remaining unsplit multis go straight into result.
    result.extend(splittable);

    // Fisher-Yates shuffle with our RNG.
    let n = result.len();
    for i in (1..n).rev() {
        let j = rng.next_bounded_i32((i + 1) as i32) as usize;
        result.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::Enchantment;
    use pumpkin_data::damage::DamageType;
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_data::{
        data_component_impl::ContainerImpl, data_component_impl::ContainerLootImpl,
        data_component_impl::CustomNameImpl, data_component_impl::FireworkExplosionShape,
        data_component_impl::FireworksImpl, data_component_impl::ItemNameImpl,
        data_component_impl::StoredEnchantmentsImpl,
    };
    use pumpkin_nbt::tag::NbtTag;
    use pumpkin_util::loot_table::{LootFireworkExplosion, LootListOperation};

    #[test]
    fn discard_item_removes_all_selected_stacks() {
        let mut stacks = vec![
            ItemStack::new(3, &Item::MAP),
            ItemStack::new(1, &Item::PAPER),
        ];
        let function = LootFunction {
            content: LootFunctionTypes::DiscardItem,
            conditions: None,
        };

        function.apply(&mut stacks, &LootContextParameters::default());

        assert!(stacks.is_empty());
    }

    #[test]
    fn set_name_custom_name_accepts_json_and_plain_strings() {
        let mut stacks = vec![ItemStack::new(1, &Item::MAP)];
        let function = LootFunction {
            content: LootFunctionTypes::SetName {
                name: r#""Exotic Map""#,
                target: LootNameTarget::CustomName,
            },
            conditions: None,
        };

        function.apply(&mut stacks, &LootContextParameters::default());

        let name = stacks[0]
            .get_data_component::<CustomNameImpl>()
            .expect("set_name installs custom_name")
            .name
            .clone()
            .get_text();
        assert_eq!(name, "Exotic Map");

        // The translate-object form must also parse instead of falling back to raw JSON.
        let mut stacks = vec![ItemStack::new(1, &Item::MAP)];
        let function = LootFunction {
            content: LootFunctionTypes::SetName {
                name: r#"{"translate":"filled_map.buried_treasure"}"#,
                target: LootNameTarget::CustomName,
            },
            conditions: None,
        };
        function.apply(&mut stacks, &LootContextParameters::default());
        assert!(stacks[0].get_data_component::<CustomNameImpl>().is_some());
    }

    #[test]
    fn set_name_item_name_extracts_translate_key() {
        let mut stacks = vec![ItemStack::new(1, &Item::MAP)];
        let function = LootFunction {
            content: LootFunctionTypes::SetName {
                name: r#"{"translate":"filled_map.buried_treasure"}"#,
                target: LootNameTarget::ItemName,
            },
            conditions: None,
        };

        function.apply(&mut stacks, &LootContextParameters::default());

        let component = stacks[0]
            .get_data_component::<ItemNameImpl>()
            .expect("set_name installs item_name");
        assert_eq!(component.name, "filled_map.buried_treasure");
    }

    #[test]
    fn set_container_contents_fills_slots_and_skips_empty_stacks() {
        let entries: &'static [LootPoolEntry] = &[LootPoolEntry {
            content: LootPoolEntryTypes::Item(pumpkin_util::loot_table::ItemEntry {
                name: "minecraft:diamond",
            }),
            weight: 1,
            quality: 0,
            conditions: None,
            functions: None,
        }];
        let function = LootFunction {
            content: LootFunctionTypes::SetContainerContents { entries },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::CHEST), ItemStack::EMPTY.clone()];

        function.apply(&mut stacks, &LootContextParameters::default());

        let container = stacks[0]
            .get_data_component::<ContainerImpl>()
            .expect("set_contents installs the container component");
        assert_eq!(container.items.len(), 1);
        assert_eq!(container.items[0].0, 0);
        assert_eq!(container.items[0].1.item.registry_key, "diamond");
        assert_eq!(container.items[0].1.item_count, 1);
        assert!(stacks[1].get_data_component::<ContainerImpl>().is_none());
    }

    #[test]
    fn stack_splitter_passes_through_a_stack_below_max() {
        let mut out = Vec::new();
        push_split_stack(&mut out, ItemStack::new(40, &Item::COBBLESTONE));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].item_count, 40);
    }

    #[test]
    fn set_item_preserves_count_and_changes_item() {
        let mut stacks = vec![ItemStack::new(3, &Item::DIRT)];
        let function = LootFunction {
            content: LootFunctionTypes::SetItem {
                item: "minecraft:stone",
            },
            conditions: None,
        };

        function.apply(&mut stacks, &LootContextParameters::default());

        assert_eq!(stacks[0].item.registry_key, "stone");
        assert_eq!(stacks[0].item_count, 3);
    }

    #[test]
    fn set_item_damage_matches_vanilla_fraction_and_add_mode() {
        let mut stacks = vec![ItemStack::new(1, &Item::DIAMOND_SWORD)];
        let function = LootFunction {
            content: LootFunctionTypes::SetItemDamage {
                damage: LootFunctionNumberProvider::Constant { value: 0.25 },
                add: false,
            },
            conditions: None,
        };

        function.apply(&mut stacks, &LootContextParameters::default());
        assert_eq!(stacks[0].get_damage(), 1170);

        stacks[0].set_damage(1500);
        let function = LootFunction {
            content: LootFunctionTypes::SetItemDamage {
                damage: LootFunctionNumberProvider::Constant { value: 0.1 },
                add: true,
            },
            conditions: None,
        };
        function.apply(&mut stacks, &LootContextParameters::default());
        assert_eq!(stacks[0].get_damage(), 1343);
    }

    #[test]
    fn set_container_loot_table_installs_component_and_skips_empty_stack() {
        let function = LootFunction {
            content: LootFunctionTypes::SetContainerLootTable {
                name: "minecraft:chests/simple_dungeon",
                seed: 42,
            },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::CHEST), ItemStack::EMPTY.clone()];

        function.apply(&mut stacks, &LootContextParameters::default());

        let loot = stacks[0]
            .get_data_component::<ContainerLootImpl>()
            .expect("set_loot_table installs container loot");
        assert_eq!(loot.loot_table, "minecraft:chests/simple_dungeon");
        assert_eq!(loot.seed, 42);
        assert!(
            stacks[1]
                .get_data_component::<ContainerLootImpl>()
                .is_none()
        );
    }

    #[test]
    fn set_enchantments_transmutes_books_and_clamps_levels() {
        let function = LootFunction {
            content: LootFunctionTypes::SetEnchantments {
                enchantments: &[(
                    "minecraft:sharpness",
                    LootFunctionNumberProvider::Constant { value: 300.0 },
                )],
                add: false,
            },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::BOOK)];

        function.apply(&mut stacks, &LootContextParameters::default());

        assert_eq!(stacks[0].item.id, Item::ENCHANTED_BOOK.id);
        let stored = stacks[0]
            .get_data_component::<StoredEnchantmentsImpl>()
            .expect("book enchantment is stored on the enchanted book");
        assert_eq!(stored.enchantment[0].1, 255);
    }

    #[test]
    fn set_enchantments_add_mode_updates_existing_level() {
        let function = LootFunction {
            content: LootFunctionTypes::SetEnchantments {
                enchantments: &[(
                    "minecraft:sharpness",
                    LootFunctionNumberProvider::Constant { value: 2.0 },
                )],
                add: true,
            },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::DIAMOND_SWORD)];
        stacks[0].enchant(&Enchantment::SHARPNESS, 3);

        function.apply(&mut stacks, &LootContextParameters::default());

        assert_eq!(stacks[0].get_enchantment_level(&Enchantment::SHARPNESS), 5);
    }

    #[test]
    fn set_fireworks_applies_values_and_flight_duration() {
        let mut stacks = vec![ItemStack::new(1, &Item::FIREWORK_ROCKET)];
        let function = LootFunction {
            content: LootFunctionTypes::SetFireworks {
                explosions: Some(LootFireworkExplosionOperation {
                    values: &[LootFireworkExplosion {
                        shape: "star",
                        colors: &[11_743_532],
                        fade_colors: &[],
                        has_trail: true,
                        has_twinkle: false,
                    }],
                    operation: LootListOperation::ReplaceAll,
                }),
                flight_duration: Some(2),
            },
            conditions: None,
        };

        function.apply(&mut stacks, &LootContextParameters::default());

        let fireworks = stacks[0]
            .get_data_component::<FireworksImpl>()
            .expect("set_fireworks installs its component");
        assert_eq!(fireworks.flight_duration, 2);
        assert_eq!(fireworks.explosions.len(), 1);
        assert_eq!(fireworks.explosions[0].shape, FireworkExplosionShape::Star);
        assert!(fireworks.explosions[0].has_trail);
    }

    #[test]
    fn stack_splitter_splits_an_oversized_stack() {
        let max = ItemStack::new(1, &Item::COBBLESTONE).get_max_stack_size();
        assert_eq!(max, 64);
        let mut out = Vec::new();
        push_split_stack(&mut out, ItemStack::new(150, &Item::COBBLESTONE));
        assert_eq!(
            out.iter().map(|s| u32::from(s.item_count)).sum::<u32>(),
            150
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].item_count, 64);
        assert_eq!(out[1].item_count, 64);
        assert_eq!(out[2].item_count, 22);
    }

    #[test]
    fn stack_splitter_respects_a_non_default_max_stack_size() {
        let max = ItemStack::new(1, &Item::ENDER_PEARL).get_max_stack_size();
        assert_eq!(max, 16);
        let mut out = Vec::new();
        push_split_stack(&mut out, ItemStack::new(40, &Item::ENDER_PEARL));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].item_count, 16);
        assert_eq!(out[1].item_count, 16);
        assert_eq!(out[2].item_count, 8);
    }

    /// `DecoratedPotBlock.getDrops` (`DecoratedPotBlock.java:181-191`) chooses the dynamic
    /// sherd entry from the cracked state and reads all four decorations from the block entity.
    #[test]
    fn decorated_pot_dynamic_drops_use_block_entity_decorations() {
        use crate::block::entities::decorated_pot::DecoratedPotBlockEntity;
        use pumpkin_util::math::position::BlockPos;
        use std::sync::Arc;

        let pot = Arc::new(DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0)));
        futures::executor::block_on(async {
            *pot.sherds.lock().await = Some(vec![
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:brick".into()),
            ]);
        });

        let entry = LootPoolEntryTypes::Dynamic(pumpkin_util::loot_table::DynamicEntry {
            name: "minecraft:sherds",
        });
        let stacks = entry.get_stacks(&LootContextParameters {
            block_entity: Some(pot),
            ..Default::default()
        });

        assert_eq!(stacks.len(), 4);
        assert!(stacks.iter().all(|stack| stack.item == &Item::BRICK));
    }

    /// `DecoratedPotBlock.getDrops` (`DecoratedPotBlock.java:181-191`) copies the
    /// `pot_decorations` component for the uncracked pot-item branch.
    #[test]
    fn decorated_pot_copy_components_preserves_decorations() {
        use crate::block::entities::decorated_pot::DecoratedPotBlockEntity;
        use pumpkin_data::data_component_impl::PotDecorationsImpl;
        use pumpkin_util::math::position::BlockPos;
        use std::sync::Arc;

        let pot = Arc::new(DecoratedPotBlockEntity::new(BlockPos::new(0, 0, 0)));
        futures::executor::block_on(async {
            *pot.sherds.lock().await = Some(vec![
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:brick".into()),
                NbtTag::String("minecraft:brick".into()),
            ]);
        });

        let function = LootFunction {
            content: LootFunctionTypes::CopyComponents {
                source: "block_entity",
                include: &["minecraft:pot_decorations"],
            },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::DECORATED_POT)];
        function.apply(
            &mut stacks,
            &LootContextParameters {
                block_entity: Some(pot),
                ..Default::default()
            },
        );

        let decorations = stacks[0]
            .get_data_component::<PotDecorationsImpl>()
            .expect("pot decoration component should be copied");
        assert_eq!(decorations.decorations.len(), 4);
        assert!(
            decorations
                .decorations
                .iter()
                .all(|item| item == "minecraft:brick")
        );
    }

    #[test]
    fn set_book_cover_updates_only_requested_fields() {
        let function = LootFunction {
            content: LootFunctionTypes::SetBookCover {
                title: Some("Treasure"),
                author: None,
                generation: Some(2),
            },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::WRITTEN_BOOK)];

        function.apply(&mut stacks, &LootContextParameters::default());

        let content = stacks[0]
            .get_data_component::<WrittenBookContentImpl>()
            .expect("set_book_cover installs written book content");
        assert_eq!(content.title, "Treasure");
        assert_eq!(content.author, "");
        assert_eq!(content.generation, 2);
        assert!(content.pages.is_empty());
    }

    #[test]
    fn set_firework_explosion_uses_vanilla_defaults_for_missing_component() {
        let function = LootFunction {
            content: LootFunctionTypes::SetFireworkExplosion {
                shape: Some("burst"),
                colors: Some(&[0x12_3456]),
                fade_colors: None,
                trail: Some(true),
                twinkle: None,
            },
            conditions: None,
        };
        let mut stacks = vec![ItemStack::new(1, &Item::FIREWORK_STAR)];

        function.apply(&mut stacks, &LootContextParameters::default());

        let explosion = stacks[0]
            .get_data_component::<FireworkExplosionImpl>()
            .expect("set_firework_explosion installs explosion content");
        assert_eq!(explosion.shape, FireworkExplosionShape::Burst);
        assert_eq!(explosion.colors, vec![0x12_3456]);
        assert!(explosion.fade_colors.is_empty());
        assert!(explosion.has_trail);
        assert!(!explosion.has_twinkle);
    }

    /// Vanilla `TagEntry.expandTag` (`TagEntry.java:50-65`), driven from
    /// `LootPool.addRandomItem` (`LootPool.java:70-77`): an `expand: true` tag entry fans out
    /// into one weighted candidate per tag item, each carrying the entry's own weight, rather
    /// than contributing its weight once and choosing an item internally. With a 16-item tag
    /// (`minecraft:wool`) at weight 1 against a single sibling entry also at weight 1, the tag
    /// should win roughly 16-in-17 rolls, not roughly half.
    #[test]
    fn expand_tag_entry_contributes_one_candidate_per_item() {
        let tag_entry = LootPoolEntry {
            content: LootPoolEntryTypes::Tag(pumpkin_util::loot_table::TagEntry {
                name: "minecraft:wool",
                expand: true,
            }),
            weight: 1,
            quality: 0,
            conditions: None,
            functions: None,
        };
        let item_entry = LootPoolEntry {
            content: LootPoolEntryTypes::Item(pumpkin_util::loot_table::ItemEntry {
                name: "minecraft:stone",
            }),
            weight: 1,
            quality: 0,
            conditions: None,
            functions: None,
        };
        let entries: &'static [LootPoolEntry] =
            Box::leak(vec![tag_entry, item_entry].into_boxed_slice());
        let pool = pumpkin_util::loot_table::LootPool {
            entries,
            rolls: pumpkin_util::loot_table::LootNumberProviderTypes::Constant(1.0),
            bonus_rolls: pumpkin_util::loot_table::LootNumberProviderTypes::Constant(0.0),
            conditions: None,
            functions: None,
        };
        let pools: &'static [pumpkin_util::loot_table::LootPool] =
            Box::leak(vec![pool].into_boxed_slice());
        let table = LootTable {
            r#type: pumpkin_util::loot_table::LootTableType::Chest,
            random_sequence: None,
            pools: Some(pools),
        };

        let trials: u32 = 4000;
        let mut wool_rolls = 0u32;
        for _ in 0..trials {
            let loot = table.get_loot(LootContextParameters::default());
            assert_eq!(loot.len(), 1);
            if loot[0].item != &Item::STONE {
                wool_rolls += 1;
            }
        }

        // Expected ~16/17 = 0.941; the old single-candidate-per-tag behavior would land near
        // 0.5. 0.85 is a wide margin that only the correct fan-out clears.
        let wool_fraction = f64::from(wool_rolls) / f64::from(trials);
        assert!(
            wool_fraction > 0.85,
            "expected the 16-item expand tag to dominate the roll (~0.94), got {wool_fraction}"
        );
    }

    /// Pool functions decorate the selected entry's output before it is emitted, as in
    /// `LootPool.addRandomItems` (`LootPool.java:97-103`).
    #[test]
    fn pool_functions_are_applied_to_selected_loot() {
        let entry = LootPoolEntry {
            content: LootPoolEntryTypes::Item(pumpkin_util::loot_table::ItemEntry {
                name: "minecraft:stone",
            }),
            weight: 1,
            quality: 0,
            conditions: None,
            functions: None,
        };
        let entries: &'static [LootPoolEntry] = Box::leak(vec![entry].into_boxed_slice());
        let pool = pumpkin_util::loot_table::LootPool {
            entries,
            rolls: pumpkin_util::loot_table::LootNumberProviderTypes::Constant(1.0),
            bonus_rolls: pumpkin_util::loot_table::LootNumberProviderTypes::Constant(0.0),
            conditions: None,
            functions: Some(&[LootFunction {
                content: LootFunctionTypes::SetCount {
                    count: LootFunctionNumberProvider::Constant { value: 7.0 },
                    add: false,
                },
                conditions: None,
            }]),
        };
        let pools: &'static [pumpkin_util::loot_table::LootPool] =
            Box::leak(vec![pool].into_boxed_slice());
        let table = LootTable {
            r#type: pumpkin_util::loot_table::LootTableType::Chest,
            random_sequence: None,
            pools: Some(pools),
        };

        let loot = table.get_loot(LootContextParameters::default());
        assert_eq!(loot.len(), 1);
        assert_eq!(loot[0].item.id, Item::STONE.id);
        assert_eq!(loot[0].item_count, 7);
    }

    fn base_params() -> LootContextParameters {
        LootContextParameters {
            killed_by_player: Some(true),
            this_entity: Some(&EntityType::PIG),
            killer_entity: Some(&EntityType::PLAYER),
            direct_killer_entity: Some(&EntityType::PLAYER),
            damage_type: Some(DamageType::GENERIC),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn copy_components_preserves_shulker_contents() {
        // `ShulkerBoxBlock.getDrops` (`ShulkerBoxBlock.java:127-139`) copies the container
        // component into the dropped shulker box instead of scattering its contents.
        let entity: Arc<dyn BlockEntity> = Arc::new(
            crate::block::entities::shulker_box::ShulkerBoxBlockEntity::new(
                pumpkin_util::math::position::BlockPos::new(0, 64, 0),
            ),
        );
        let inventory = entity.clone().get_inventory().expect("shulker inventory");
        inventory
            .set_stack(3, ItemStack::new(5, &Item::DIAMOND))
            .await;

        let mut stacks = vec![ItemStack::new(1, &Item::SHULKER_BOX)];
        let params = LootContextParameters {
            block_entity: Some(entity),
            ..Default::default()
        };
        let function = LootFunction {
            content: LootFunctionTypes::CopyComponents {
                source: "block_entity",
                include: &["minecraft:container"],
            },
            conditions: None,
        };

        function.apply(&mut stacks, &params);

        let container = stacks[0]
            .get_data_component::<ContainerImpl>()
            .expect("container component");
        assert_eq!(container.items.len(), 1);
        assert_eq!(container.items[0].0, 3);
        assert_eq!(container.items[0].1.get_item().id, Item::DIAMOND.id);
        assert_eq!(container.items[0].1.item_count, 5);
    }

    fn fire_aspect_sword(level: i32) -> ItemStack {
        let mut sword = ItemStack::new(1, &Item::DIAMOND_SWORD);
        sword.enchant(&Enchantment::FIRE_ASPECT, level);
        sword
    }

    #[test]
    fn entity_properties_this_matches_expected_type() {
        let params = base_params();
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: Some("minecraft:pig"),
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn entity_properties_this_rejects_wrong_type() {
        let params = base_params();
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: Some("minecraft:cow"),
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn entity_properties_direct_attacker_resolves() {
        let params = base_params();
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn entity_properties_direct_attacker_no_direct_killer() {
        let mut params = base_params();
        params.direct_killer_entity = None;
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn entity_properties_unknown_entity_returns_false() {
        let params = base_params();
        let cond = LootCondition::EntityProperties {
            entity: "target_entity",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn is_on_fire_true_when_burning() {
        let params = LootContextParameters {
            is_on_fire: Some(true),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: None,
            is_on_fire: Some(true),
            mainhand_enchantment_tag: None,
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn is_on_fire_true_fails_when_not_burning() {
        let params = LootContextParameters {
            is_on_fire: Some(false),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: None,
            is_on_fire: Some(true),
            mainhand_enchantment_tag: None,
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn is_on_fire_false_matches_not_burning() {
        let params = LootContextParameters {
            is_on_fire: Some(false),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: None,
            is_on_fire: Some(false),
            mainhand_enchantment_tag: None,
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn is_on_fire_true_fails_when_context_none() {
        let params = LootContextParameters {
            is_on_fire: None,
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: None,
            is_on_fire: Some(true),
            mainhand_enchantment_tag: None,
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn none_is_on_fire_skips_check() {
        let params = LootContextParameters {
            is_on_fire: Some(true),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "this",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn enchantment_tag_matches_fire_aspect() {
        let params = LootContextParameters {
            tool: Some(fire_aspect_sword(1)),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn enchantment_tag_fails_without_enchantment() {
        let params = LootContextParameters {
            tool: Some(ItemStack::new(1, &Item::DIAMOND_SWORD)),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn enchantment_tag_rejects_unrelated_enchantment() {
        let mut sword = ItemStack::new(1, &Item::DIAMOND_SWORD);
        sword.enchant(&Enchantment::SHARPNESS, 5);
        let params = LootContextParameters {
            tool: Some(sword),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn enchantment_tag_fails_with_no_tool() {
        let params = LootContextParameters {
            tool: None,
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
        };
        assert!(!cond.is_fulfilled(&params));
    }

    #[test]
    fn none_enchantment_tag_skips_check() {
        let params = LootContextParameters {
            tool: Some(fire_aspect_sword(2)),
            ..base_params()
        };
        let cond = LootCondition::EntityProperties {
            entity: "direct_attacker",
            expected_type: None,
            is_on_fire: None,
            mainhand_enchantment_tag: None,
        };
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn anyof_passes_when_entity_on_fire() {
        let params = LootContextParameters {
            is_on_fire: Some(true),
            tool: Some(ItemStack::new(1, &Item::DIAMOND_SWORD)),
            ..base_params()
        };
        let cond = LootCondition::AnyOf(&[
            LootCondition::EntityProperties {
                entity: "this",
                expected_type: None,
                is_on_fire: Some(true),
                mainhand_enchantment_tag: None,
            },
            LootCondition::EntityProperties {
                entity: "direct_attacker",
                expected_type: None,
                is_on_fire: None,
                mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
            },
        ]);
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn anyof_passes_when_weapon_has_fire_aspect() {
        let params = LootContextParameters {
            is_on_fire: Some(false),
            tool: Some(fire_aspect_sword(1)),
            ..base_params()
        };
        let cond = LootCondition::AnyOf(&[
            LootCondition::EntityProperties {
                entity: "this",
                expected_type: None,
                is_on_fire: Some(true),
                mainhand_enchantment_tag: None,
            },
            LootCondition::EntityProperties {
                entity: "direct_attacker",
                expected_type: None,
                is_on_fire: None,
                mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
            },
        ]);
        assert!(cond.is_fulfilled(&params));
    }

    #[test]
    fn anyof_fails_without_fire_or_fire_aspect() {
        let params = LootContextParameters {
            is_on_fire: Some(false),
            tool: Some(ItemStack::new(1, &Item::DIAMOND_SWORD)),
            ..base_params()
        };
        let cond = LootCondition::AnyOf(&[
            LootCondition::EntityProperties {
                entity: "this",
                expected_type: None,
                is_on_fire: Some(true),
                mainhand_enchantment_tag: None,
            },
            LootCondition::EntityProperties {
                entity: "direct_attacker",
                expected_type: None,
                is_on_fire: None,
                mainhand_enchantment_tag: Some("minecraft:smelts_loot"),
            },
        ]);
        assert!(!cond.is_fulfilled(&params));
    }

    fn single_entry_chest_table(
        entry: pumpkin_util::chest_loot_table::ChestLootEntry,
    ) -> pumpkin_util::chest_loot_table::ChestLootTable {
        let entries: &'static [pumpkin_util::chest_loot_table::ChestLootEntry] =
            Box::leak(vec![entry].into_boxed_slice());
        let pools: &'static [pumpkin_util::chest_loot_table::ChestLootPool] = Box::leak(
            vec![pumpkin_util::chest_loot_table::ChestLootPool {
                entries,
                min_rolls: 1,
                max_rolls: 1,
                empty_weight: 0,
            }]
            .into_boxed_slice(),
        );
        pumpkin_util::chest_loot_table::ChestLootTable { pools }
    }

    /// Vanilla `EnchantRandomlyFunction.enchantItem`
    /// (`EnchantRandomlyFunction.java:89-102`): books are transmuted to enchanted
    /// books whose stored-enchantments component carries the rolled enchantment
    /// at a level within `[min_level, max_level]`.
    #[test]
    fn chest_enchant_randomly_transmutes_books_to_stored_enchantments() {
        let table = single_entry_chest_table(pumpkin_util::chest_loot_table::ChestLootEntry {
            item: "minecraft:book",
            weight: 1,
            min_count: 1,
            max_count: 1,
            enchant_randomly: Some(&["minecraft:sharpness"]),
        });

        let loot = generate_chest_loot(&table, 42);

        assert_eq!(loot.len(), 1);
        assert_eq!(loot[0].item.id, Item::ENCHANTED_BOOK.id);
        let stored = loot[0]
            .get_data_component::<StoredEnchantmentsImpl>()
            .expect("enchanted book carries stored enchantments");
        assert_eq!(stored.enchantment[0].0.id, Enchantment::SHARPNESS.id);
        assert!(
            (1..=Enchantment::SHARPNESS.max_level).contains(&stored.enchantment[0].1),
            "level {} outside the enchantment's bounds",
            stored.enchantment[0].1
        );
    }

    /// Vanilla `EnchantRandomlyFunction.run` (:73-78): non-book targets keep the
    /// default `only_compatible` filter, so an incompatible candidate is dropped;
    /// the enchantment lands in the regular enchantments component (:96).
    #[test]
    fn chest_enchant_randomly_filters_incompatible_candidates_for_gear() {
        let table = single_entry_chest_table(pumpkin_util::chest_loot_table::ChestLootEntry {
            item: "minecraft:golden_axe",
            weight: 1,
            min_count: 1,
            max_count: 1,
            // Protection is armor-only and must be filtered out for an axe.
            enchant_randomly: Some(&["minecraft:protection", "minecraft:efficiency"]),
        });

        let loot = generate_chest_loot(&table, 7);

        assert_eq!(loot.len(), 1);
        assert_eq!(loot[0].item.registry_key, "golden_axe");
        let efficiency_level = loot[0].get_enchantment_level(&Enchantment::EFFICIENCY);
        assert!(
            (1..=Enchantment::EFFICIENCY.max_level).contains(&efficiency_level),
            "efficiency level {efficiency_level} outside its bounds"
        );
        assert_eq!(loot[0].get_enchantment_level(&Enchantment::PROTECTION), 0);
    }

    /// A `#`-prefixed `options` value resolves through its enchantment tag's members.
    #[test]
    fn chest_enchant_randomly_expands_tag_options() {
        let table = single_entry_chest_table(pumpkin_util::chest_loot_table::ChestLootEntry {
            item: "minecraft:diamond_sword",
            weight: 1,
            min_count: 1,
            max_count: 1,
            // The smelts-loot tag holds exactly fire_aspect, which applies to swords.
            enchant_randomly: Some(&["#minecraft:smelts_loot"]),
        });

        let loot = generate_chest_loot(&table, 3);

        assert_eq!(loot.len(), 1);
        let fire_aspect_level = loot[0].get_enchantment_level(&Enchantment::FIRE_ASPECT);
        assert!(
            (1..=Enchantment::FIRE_ASPECT.max_level).contains(&fire_aspect_level),
            "fire_aspect level {fire_aspect_level} outside its bounds"
        );
    }
}
