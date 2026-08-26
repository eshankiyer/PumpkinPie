use super::BlockEntity;
use crate::entity::Entity;
use crate::entity::item::ItemEntity;
use crate::world::World;
use crate::world::loot::generate_chest_loot;
use pumpkin_data::block_properties::{BlockProperties, SuspiciousSandLikeProperties};
use pumpkin_data::chest_loot_table::{ChestLootEntry, ChestLootPool, ChestLootTable};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

/// `BrushableBlockEntity.java:44-46`.
const BRUSH_COOLDOWN_TICKS: i64 = 10;
const BRUSH_RESET_TICKS: i64 = 40;
const REQUIRED_BRUSHES_TO_BREAK: i32 = 10;
/// `BrushableBlock.java:39` (`TICK_DELAY`).
const TICK_DELAY: u8 = 2;
/// `BrushableBlockEntity.java:157` (`retractionSpeed`).
const RETRACTION_SPEED: i64 = 4;

const fn loot_item(name: &'static str, weight: i32) -> ChestLootEntry {
    ChestLootEntry {
        item: name,
        weight,
        min_count: 1,
        max_count: 1,
        enchant_randomly: None,
    }
}

const fn single_pool(entries: &'static [ChestLootEntry]) -> ChestLootPool {
    ChestLootPool {
        entries,
        min_rolls: 1,
        max_rolls: 1,
        empty_weight: 0,
    }
}

static DESERT_PYRAMID_POOLS: &[ChestLootPool] = &[single_pool(&[
    loot_item("minecraft:archer_pottery_sherd", 1),
    loot_item("minecraft:miner_pottery_sherd", 1),
    loot_item("minecraft:prize_pottery_sherd", 1),
    loot_item("minecraft:skull_pottery_sherd", 1),
    loot_item("minecraft:diamond", 1),
    loot_item("minecraft:tnt", 1),
    loot_item("minecraft:gunpowder", 1),
    loot_item("minecraft:emerald", 1),
])];

// The `minecraft:suspicious_stew` entry carries a `set_stew_effect` function that is not
// modelled here, so the stew drops without its effect component.
static DESERT_WELL_POOLS: &[ChestLootPool] = &[single_pool(&[
    loot_item("minecraft:arms_up_pottery_sherd", 2),
    loot_item("minecraft:brewer_pottery_sherd", 2),
    loot_item("minecraft:brick", 1),
    loot_item("minecraft:emerald", 1),
    loot_item("minecraft:stick", 1),
    loot_item("minecraft:suspicious_stew", 1),
])];

static OCEAN_RUIN_COLD_POOLS: &[ChestLootPool] = &[single_pool(&[
    loot_item("minecraft:blade_pottery_sherd", 1),
    loot_item("minecraft:explorer_pottery_sherd", 1),
    loot_item("minecraft:mourner_pottery_sherd", 1),
    loot_item("minecraft:plenty_pottery_sherd", 1),
    loot_item("minecraft:iron_axe", 1),
    loot_item("minecraft:emerald", 2),
    loot_item("minecraft:wheat", 2),
    loot_item("minecraft:wooden_hoe", 2),
    loot_item("minecraft:coal", 2),
    loot_item("minecraft:gold_nugget", 2),
])];

static OCEAN_RUIN_WARM_POOLS: &[ChestLootPool] = &[single_pool(&[
    loot_item("minecraft:angler_pottery_sherd", 1),
    loot_item("minecraft:shelter_pottery_sherd", 1),
    loot_item("minecraft:snort_pottery_sherd", 1),
    loot_item("minecraft:sniffer_egg", 1),
    loot_item("minecraft:iron_axe", 1),
    loot_item("minecraft:emerald", 2),
    loot_item("minecraft:wheat", 2),
    loot_item("minecraft:wooden_hoe", 2),
    loot_item("minecraft:coal", 2),
    loot_item("minecraft:gold_nugget", 2),
])];

static TRAIL_RUINS_COMMON_POOLS: &[ChestLootPool] = &[single_pool(&[
    loot_item("minecraft:emerald", 2),
    loot_item("minecraft:wheat", 2),
    loot_item("minecraft:wooden_hoe", 2),
    loot_item("minecraft:clay", 2),
    loot_item("minecraft:brick", 2),
    loot_item("minecraft:yellow_dye", 2),
    loot_item("minecraft:blue_dye", 2),
    loot_item("minecraft:light_blue_dye", 2),
    loot_item("minecraft:white_dye", 2),
    loot_item("minecraft:orange_dye", 2),
    loot_item("minecraft:red_candle", 2),
    loot_item("minecraft:green_candle", 2),
    loot_item("minecraft:purple_candle", 2),
    loot_item("minecraft:brown_candle", 2),
    loot_item("minecraft:magenta_stained_glass_pane", 1),
    loot_item("minecraft:pink_stained_glass_pane", 1),
    loot_item("minecraft:blue_stained_glass_pane", 1),
    loot_item("minecraft:light_blue_stained_glass_pane", 1),
    loot_item("minecraft:red_stained_glass_pane", 1),
    loot_item("minecraft:yellow_stained_glass_pane", 1),
    loot_item("minecraft:purple_stained_glass_pane", 1),
    loot_item("minecraft:spruce_hanging_sign", 1),
    loot_item("minecraft:oak_hanging_sign", 1),
    loot_item("minecraft:gold_nugget", 1),
    loot_item("minecraft:coal", 1),
    loot_item("minecraft:wheat_seeds", 1),
    loot_item("minecraft:beetroot_seeds", 1),
    loot_item("minecraft:dead_bush", 1),
    loot_item("minecraft:flower_pot", 1),
    loot_item("minecraft:string", 1),
    loot_item("minecraft:lead", 1),
])];

static TRAIL_RUINS_RARE_POOLS: &[ChestLootPool] = &[single_pool(&[
    loot_item("minecraft:burn_pottery_sherd", 1),
    loot_item("minecraft:danger_pottery_sherd", 1),
    loot_item("minecraft:friend_pottery_sherd", 1),
    loot_item("minecraft:heart_pottery_sherd", 1),
    loot_item("minecraft:heartbreak_pottery_sherd", 1),
    loot_item("minecraft:howl_pottery_sherd", 1),
    loot_item("minecraft:sheaf_pottery_sherd", 1),
    loot_item("minecraft:wayfinder_armor_trim_smithing_template", 1),
    loot_item("minecraft:raiser_armor_trim_smithing_template", 1),
    loot_item("minecraft:shaper_armor_trim_smithing_template", 1),
    loot_item("minecraft:host_armor_trim_smithing_template", 1),
    loot_item("minecraft:music_disc_relic", 1),
])];

/// The six `data/minecraft/loot_table/archaeology/*.json` tables, transcribed from
/// `assets/datapacks/26_2`. They are absent from the generated chest-loot registry,
/// which only carries `chests/*`.
///
/// Every one of them is a single pool with `rolls: 1`, no count functions and default
/// weights, so the chest-loot roller reproduces them exactly.
#[must_use]
pub fn archaeology_loot_table(key: &str) -> Option<ChestLootTable> {
    let key = key.strip_prefix("minecraft:").unwrap_or(key);
    let pools = match key {
        "archaeology/desert_pyramid" => DESERT_PYRAMID_POOLS,
        "archaeology/desert_well" => DESERT_WELL_POOLS,
        "archaeology/ocean_ruin_cold" => OCEAN_RUIN_COLD_POOLS,
        "archaeology/ocean_ruin_warm" => OCEAN_RUIN_WARM_POOLS,
        "archaeology/trail_ruins_common" => TRAIL_RUINS_COMMON_POOLS,
        "archaeology/trail_ruins_rare" => TRAIL_RUINS_RARE_POOLS,
        _ => return None,
    };
    Some(ChestLootTable { pools })
}

/// `BrushableBlockEntity.getCompletionState` (`BrushableBlockEntity.java:230-238`).
#[must_use]
pub const fn completion_state(brush_count: i32) -> u8 {
    if brush_count == 0 {
        0
    } else if brush_count < 3 {
        1
    } else if brush_count < 6 {
        2
    } else {
        3
    }
}

/// `BrushableBlock`'s `turns_into` codec field, as bound by the two vanilla instances
/// registered for suspicious sand and gravel.
#[must_use]
pub fn turns_into(block: &Block) -> &'static Block {
    if block.id == BlockId::SUSPICIOUS_GRAVEL {
        &Block::GRAVEL
    } else {
        &Block::SAND
    }
}

/// All mutable state, behind a single lock: two players brushing the same block must not
/// interleave a read-modify-write of the counters.
struct BrushState {
    brush_count: i32,
    brush_count_resets_at_tick: i64,
    cool_down_ends_at_tick: i64,
    item: Option<ItemStack>,
    hit_direction: Option<BlockDirection>,
    loot_table: Option<String>,
    loot_table_seed: i64,
}

impl BrushState {
    const fn new() -> Self {
        Self {
            brush_count: 0,
            brush_count_resets_at_tick: 0,
            cool_down_ends_at_tick: 0,
            item: None,
            hit_direction: None,
            loot_table: None,
            loot_table_seed: 0,
        }
    }

    /// `BrushableBlockEntity.unpackLootTable` (`BrushableBlockEntity.java:88-114`).
    ///
    /// The `GENERATE_LOOT` advancement trigger and the luck / `TOOL` / `THIS_ENTITY` loot
    /// parameters have no analogue here; none of the six archaeology tables reads them.
    fn unpack_loot_table(&mut self) {
        let Some(key) = self.loot_table.take() else {
            return;
        };
        let Some(table) = archaeology_loot_table(&key) else {
            self.item = None;
            return;
        };
        let seed = if self.loot_table_seed == 0 {
            rand::random()
        } else {
            self.loot_table_seed
        };
        let mut loot = generate_chest_loot(&table, seed);
        // Vanilla warns and keeps the first stack when a table yields more than one.
        self.item = if loot.is_empty() {
            None
        } else {
            Some(loot.swap_remove(0))
        };
    }
}

pub struct BrushableBlockBlockEntity {
    pub position: BlockPos,
    state: Mutex<BrushState>,
}

impl BlockEntity for BrushableBlockBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    /// `BrushableBlockEntity.loadAdditional` (`BrushableBlockEntity.java:205-215`): a
    /// pending loot table wins over any stored item.
    fn from_nbt(nbt: &NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let loot_table = nbt.get_string("LootTable").map(ToString::to_string);
        let loot_table_seed = nbt.get_long("LootTableSeed").unwrap_or(0);
        let item = if loot_table.is_some() {
            None
        } else {
            nbt.get_compound("item")
                .and_then(ItemStack::read_item_stack)
        };
        let hit_direction = nbt
            .get_byte("hit_direction")
            .and_then(|b| u8::try_from(b).ok())
            .and_then(BlockDirection::from_index);

        Self {
            position,
            state: Mutex::new(BrushState {
                item,
                hit_direction,
                loot_table,
                loot_table_seed,
                ..BrushState::new()
            }),
        }
    }

    /// `BrushableBlockEntity.saveAdditional` (`BrushableBlockEntity.java:217-223`): the
    /// rolled item is written only when no deferred loot table remains.
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let state = self.state.lock().await;
            write_state(&state, nbt);
        })
    }

    /// `BrushableBlockEntity.getUpdateTag` (`BrushableBlockEntity.java:189-199`): the
    /// client is told the hit direction and the revealed item, never the counters.
    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let state = self.state.try_lock().ok()?;
        let mut nbt = NbtCompound::new();
        if let Some(direction) = state.hit_direction {
            nbt.put_byte("hit_direction", direction.to_index() as i8);
        }
        if let Some(stack) = state.item.as_ref() {
            let mut item_nbt = NbtCompound::new();
            stack.write_item_stack(&mut item_nbt);
            nbt.put_compound("item", item_nbt);
        }
        Some(nbt)
    }

    fn take_loot_table(&self) -> Option<(String, i64)> {
        let mut state = self.state.try_lock().ok()?;
        let seed = state.loot_table_seed;
        state.loot_table.take().map(|key| (key, seed))
    }

    fn has_loot_table(&self) -> bool {
        self.state
            .try_lock()
            .is_ok_and(|state| state.loot_table.is_some())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn write_state(state: &BrushState, nbt: &mut NbtCompound) {
    if let Some(key) = state.loot_table.as_ref() {
        nbt.put_string("LootTable", key.clone());
        if state.loot_table_seed != 0 {
            nbt.put_long("LootTableSeed", state.loot_table_seed);
        }
    } else if let Some(stack) = state.item.as_ref() {
        let mut item_nbt = NbtCompound::new();
        stack.write_item_stack(&mut item_nbt);
        nbt.put_compound("item", item_nbt);
    }
    if let Some(direction) = state.hit_direction {
        nbt.put_byte("hit_direction", direction.to_index() as i8);
    }
}

enum BrushOutcome {
    /// Cooldown, or a stroke that did not change the `dusted` stage.
    Unchanged,
    Dusted(u8),
    Completed(Option<ItemStack>, BlockDirection),
}

impl BrushableBlockBlockEntity {
    pub const ID: &'static str = "minecraft:brushable_block";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            state: Mutex::const_new(BrushState::new()),
        }
    }

    /// `BrushableBlockEntity.setLootTable` (`BrushableBlockEntity.java:225-228`).
    pub async fn set_loot_table(&self, key: String, seed: i64) {
        let mut state = self.state.lock().await;
        state.loot_table = Some(key);
        state.loot_table_seed = seed;
        state.item = None;
    }

    /// Unpacks any pending loot table and takes the contained item, for the drop that
    /// happens when the block is mined rather than brushed away.
    pub async fn take_item(&self) -> Option<ItemStack> {
        let mut state = self.state.lock().await;
        state.unpack_loot_table();
        state.item.take()
    }

    /// `BrushableBlockEntity.brush` (`BrushableBlockEntity.java:59-86`). Returns `true`
    /// when the block was fully brushed away, which is what makes the caller damage the
    /// brush (`BrushItem.java:81-87`).
    pub async fn brush(
        &self,
        world: &Arc<World>,
        game_time: i64,
        direction: BlockDirection,
    ) -> bool {
        let block = world.get_block(&self.position);

        let outcome = {
            let mut state = self.state.lock().await;
            if state.hit_direction.is_none() {
                state.hit_direction = Some(direction);
            }
            // Set before the cooldown check: a stroke landing inside the cooldown still
            // postpones the decay (`BrushableBlockEntity.java:64-67`).
            state.brush_count_resets_at_tick = game_time + BRUSH_RESET_TICKS;
            if game_time < state.cool_down_ends_at_tick {
                return false;
            }
            state.cool_down_ends_at_tick = game_time + BRUSH_COOLDOWN_TICKS;
            state.unpack_loot_table();

            let previous = completion_state(state.brush_count);
            state.brush_count += 1;
            if state.brush_count >= REQUIRED_BRUSHES_TO_BREAK {
                state.unpack_loot_table();
                let hit_direction = state.hit_direction.unwrap_or(BlockDirection::Up);
                BrushOutcome::Completed(state.item.take(), hit_direction)
            } else {
                let current = completion_state(state.brush_count);
                if previous == current {
                    BrushOutcome::Unchanged
                } else {
                    BrushOutcome::Dusted(current)
                }
            }
        };

        match outcome {
            BrushOutcome::Completed(item, hit_direction) => {
                self.brushing_completed(world, block, item, hit_direction)
                    .await;
                true
            }
            other => {
                world.schedule_block_tick(block, self.position, TICK_DELAY, TickPriority::Normal);
                if let BrushOutcome::Dusted(stage) = other {
                    set_dusted(world, &self.position, block, stage).await;
                }
                false
            }
        }
    }

    /// `BrushableBlockEntity.brushingCompleted` and `dropContent`
    /// (`BrushableBlockEntity.java:116-146`).
    async fn brushing_completed(
        &self,
        world: &Arc<World>,
        block: &'static Block,
        item: Option<ItemStack>,
        hit_direction: BlockDirection,
    ) {
        if let Some(stack) = item {
            let drop_pos = self.position.offset(hit_direction.to_offset());
            let size = f64::from(EntityType::ITEM.dimension[0]);
            let center_range = 1.0 - size;
            let half_size = size / 2.0;
            let spawn_pos = Vector3::new(
                f64::from(drop_pos.0.x) + 0.5 * center_range + half_size,
                f64::from(drop_pos.0.y) + 0.5 + f64::from(EntityType::ITEM.dimension[1]) / 2.0,
                f64::from(drop_pos.0.z) + 0.5 * center_range + half_size,
            );
            let entity = Entity::new(world.clone(), spawn_pos, &EntityType::ITEM);
            world
                .spawn_entity(Arc::new(ItemEntity::new(entity, stack)))
                .await;
        }

        // Vanilla plays no sound server-side here: level event 3008 is what makes the
        // client play the block's `brush_completed_sound`
        // (`BrushableBlockEntity.java:119`).
        let state_id = world.get_block_state_id(&self.position);
        world.sync_world_event(
            WorldEvent::ParticlesAndSoundBrushBlockComplete,
            self.position,
            state_id.as_u16().into(),
        );

        world
            .set_block_state(
                &self.position,
                turns_into(block).default_state.id,
                BlockFlags::NOTIFY_ALL,
            )
            .await;
    }

    /// `BrushableBlockEntity.checkReset` (`BrushableBlockEntity.java:148-168`).
    pub async fn check_reset(&self, world: &Arc<World>, game_time: i64) {
        let block = world.get_block(&self.position);

        let (new_stage, reschedule) = {
            let mut state = self.state.lock().await;
            let mut new_stage = None;
            if state.brush_count != 0 && game_time >= state.brush_count_resets_at_tick {
                let previous = completion_state(state.brush_count);
                state.brush_count = (state.brush_count - 2).max(0);
                let current = completion_state(state.brush_count);
                if previous != current {
                    new_stage = Some(current);
                }
                state.brush_count_resets_at_tick = game_time + RETRACTION_SPEED;
            }

            if state.brush_count == 0 {
                state.hit_direction = None;
                state.brush_count_resets_at_tick = 0;
                state.cool_down_ends_at_tick = 0;
                (new_stage, false)
            } else {
                (new_stage, true)
            }
        };

        if let Some(stage) = new_stage {
            set_dusted(world, &self.position, block, stage).await;
        }
        if reschedule {
            world.schedule_block_tick(block, self.position, TICK_DELAY, TickPriority::Normal);
        }
    }
}

async fn set_dusted(world: &Arc<World>, position: &BlockPos, block: &'static Block, stage: u8) {
    let state_id = world.get_block_state_id(position);
    let mut props = SuspiciousSandLikeProperties::from_state_id(state_id, block);
    props.dusted = stage;
    world
        .set_block_state(position, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::item::Item;

    const ALL_TABLES: &[&str] = &[
        "minecraft:archaeology/desert_pyramid",
        "minecraft:archaeology/desert_well",
        "minecraft:archaeology/ocean_ruin_cold",
        "minecraft:archaeology/ocean_ruin_warm",
        "minecraft:archaeology/trail_ruins_common",
        "minecraft:archaeology/trail_ruins_rare",
    ];

    /// Catches a transcription typo in the statics above: every entry must name a real
    /// item, or the roll silently yields nothing.
    #[test]
    fn every_archaeology_entry_resolves_to_a_real_item() {
        for key in ALL_TABLES {
            let table = archaeology_loot_table(key).expect("table must exist");
            let mut entries = 0;
            for pool in table.pools {
                for entry in pool.entries {
                    let short = entry.item.strip_prefix("minecraft:").unwrap_or(entry.item);
                    assert!(
                        Item::from_registry_key(short).is_some(),
                        "{key}: unknown item {}",
                        entry.item
                    );
                    entries += 1;
                }
            }
            assert!(entries > 0, "{key}: no entries");
        }
    }

    #[test]
    fn archaeology_tables_are_keyed_with_and_without_namespace() {
        assert!(archaeology_loot_table("archaeology/desert_well").is_some());
        assert!(archaeology_loot_table("minecraft:archaeology/desert_well").is_some());
        assert!(archaeology_loot_table("chests/desert_pyramid").is_none());
    }

    /// Every archaeology table rolls exactly one item, which is what lets the block
    /// entity store a single `ItemStack` (`BrushableBlockEntity.java:103-110`).
    #[test]
    fn each_archaeology_roll_yields_exactly_one_stack() {
        for key in ALL_TABLES {
            let table = archaeology_loot_table(key).unwrap();
            for seed in [1i64, 7, 12345, -9] {
                let loot = generate_chest_loot(&table, seed);
                assert_eq!(
                    loot.len(),
                    1,
                    "{key} with seed {seed} yielded {} stacks",
                    loot.len()
                );
            }
        }
    }

    /// `BrushableBlockEntity.getCompletionState` (`BrushableBlockEntity.java:230-238`).
    #[test]
    fn completion_state_matches_vanilla_thresholds() {
        assert_eq!(completion_state(0), 0);
        assert_eq!(completion_state(1), 1);
        assert_eq!(completion_state(2), 1);
        assert_eq!(completion_state(3), 2);
        assert_eq!(completion_state(5), 2);
        assert_eq!(completion_state(6), 3);
        assert_eq!(completion_state(9), 3);
    }

    #[test]
    fn ten_brushes_are_required_to_break() {
        assert_eq!(REQUIRED_BRUSHES_TO_BREAK, 10);
        assert_eq!(BRUSH_COOLDOWN_TICKS, 10);
        assert_eq!(BRUSH_RESET_TICKS, 40);
    }

    /// `checkReset` retracts two brushes at a time and re-arms four ticks later
    /// (`BrushableBlockEntity.java:148-159`).
    #[test]
    fn decay_retracts_two_brushes_per_step() {
        let mut count = 9;
        let mut steps = 0;
        while count != 0 {
            count = (count - 2).max(0);
            steps += 1;
            assert!(steps < 10, "decay did not terminate");
        }
        assert_eq!(steps, 5);
        assert_eq!(RETRACTION_SPEED, 4);
    }

    #[test]
    fn nbt_round_trip_keeps_loot_table_and_seed() {
        let pos = BlockPos::new(1, 2, 3);
        let mut nbt = NbtCompound::new();
        nbt.put_string("LootTable", "minecraft:archaeology/desert_well".to_string());
        nbt.put_long("LootTableSeed", 42);
        nbt.put_byte("hit_direction", BlockDirection::North.to_index() as i8);

        let entity = BrushableBlockBlockEntity::from_nbt(&nbt, pos);
        let mut out = NbtCompound::new();
        let state = entity.state.try_lock().unwrap();
        assert!(state.item.is_none(), "loot table must win over item");
        write_state(&state, &mut out);
        drop(state);

        assert_eq!(
            out.get_string("LootTable").map(ToString::to_string),
            Some("minecraft:archaeology/desert_well".to_string())
        );
        assert_eq!(out.get_long("LootTableSeed"), Some(42));
        assert_eq!(
            out.get_byte("hit_direction"),
            Some(BlockDirection::North.to_index() as i8)
        );
        // The non-vanilla `hits` / `direction` keys are gone.
        assert!(out.get_int("hits").is_none());
        assert!(out.get_byte("direction").is_none());
    }

    /// A zero seed is not written back, matching `trySaveLootTable`
    /// (`BrushableBlockEntity.java:176-187`).
    #[test]
    fn zero_loot_table_seed_is_not_written() {
        let mut nbt = NbtCompound::new();
        nbt.put_string("LootTable", "minecraft:archaeology/desert_well".to_string());
        let entity = BrushableBlockBlockEntity::from_nbt(&nbt, BlockPos::new(0, 0, 0));

        let mut out = NbtCompound::new();
        write_state(&entity.state.try_lock().unwrap(), &mut out);
        assert!(out.get_string("LootTable").is_some());
        assert!(out.get_long("LootTableSeed").is_none());
    }

    #[test]
    fn unpacking_a_loot_table_produces_one_item_and_clears_the_key() {
        let mut state = BrushState::new();
        state.loot_table = Some("minecraft:archaeology/trail_ruins_rare".to_string());
        state.loot_table_seed = 99;
        state.unpack_loot_table();
        assert!(state.loot_table.is_none());
        assert!(state.item.is_some());

        // Unpacking again is a no-op, so the item is not re-rolled.
        let first = state.item.clone();
        state.unpack_loot_table();
        assert_eq!(
            state.item.as_ref().map(|s| s.item.id),
            first.map(|s| s.item.id)
        );
    }

    /// No loot table and no item means nothing drops: there is no random fallback.
    #[test]
    fn missing_loot_table_leaves_no_item() {
        let mut state = BrushState::new();
        state.loot_table = Some("minecraft:archaeology/does_not_exist".to_string());
        state.unpack_loot_table();
        assert!(state.item.is_none());
        assert!(state.loot_table.is_none());
    }

    #[test]
    fn turns_into_matches_the_two_vanilla_instances() {
        assert_eq!(turns_into(&Block::SUSPICIOUS_SAND).id, Block::SAND.id);
        assert_eq!(turns_into(&Block::SUSPICIOUS_GRAVEL).id, Block::GRAVEL.id);
    }
}
