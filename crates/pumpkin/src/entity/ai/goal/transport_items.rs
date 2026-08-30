// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use super::{Controls, Goal, GoalFuture};
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::copper_golem::{CopperGolemEntity, CopperGolemState};
use crate::world::World;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, item_stack::ItemStack};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::inventory::Inventory;
use rand::RngExt;

/// Ports the item-fetching half of vanilla's `TransportItemsBetweenContainers` behavior
/// (`CopperGolemAi.initIdleActivity`): a copper golem walks to a nearby copper chest,
/// takes one item, then walks it to a nearby chest/trapped chest and drops it in.
///
/// Search radii (32 blocks horizontal, 8 vertical) match
/// `CopperGolemAi.TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS` / `_VERTICAL_SEARCH_RADIUS`.
///
/// Deferred vs. vanilla: the real behavior queues on contested containers via
/// `ContainerOpenersCounter`/`shouldQueueForTarget` and plays a five-state open/close
/// animation (`CopperGolemState`) timed to ticks 1/9/60 after arrival. Pumpkin has no
/// `ContainerUser` plumbing for mobs, so this goal performs the pickup/drop instantly on
/// arrival instead of over that animation window.
const HORIZONTAL_RADIUS: i32 = 32;
const VERTICAL_RADIUS: i32 = 8;
const CHUNK_RADIUS: i32 = (HORIZONTAL_RADIUS + 15) / 16;

enum Phase {
    Idle,
    ToSource(BlockPos),
    ToDest { source: BlockPos, dest: BlockPos },
}

pub struct TransportItemsGoal {
    goal_control: Controls,
    speed: f64,
    phase: Phase,
    carried: Option<ItemStack>,
    cooldown: i32,
}

impl TransportItemsGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self {
            goal_control: Controls::MOVE,
            speed,
            phase: Phase::Idle,
            carried: None,
            cooldown: 0,
        }
    }

    fn is_source_block(world: &World, pos: &BlockPos) -> bool {
        world
            .get_block(pos)
            .has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS)
    }

    fn is_dest_block(world: &World, pos: &BlockPos) -> bool {
        let block = world.get_block(pos);
        block == &Block::CHEST || block == &Block::TRAPPED_CHEST
    }

    async fn find_source(world: &World, origin: BlockPos) -> Option<BlockPos> {
        Self::find_nearest(world, origin, Self::is_source_block, |be| async move {
            let Some(inventory) = be.get_inventory() else {
                return false;
            };
            for i in 0..inventory.size() {
                let stack = inventory.get_stack(i).await;
                if !stack.is_empty() {
                    return true;
                }
            }
            false
        })
        .await
    }

    async fn find_dest(world: &World, origin: BlockPos) -> Option<BlockPos> {
        Self::find_nearest(world, origin, Self::is_dest_block, |be| async move {
            let Some(inventory) = be.get_inventory() else {
                return false;
            };
            for i in 0..inventory.size() {
                let stack = inventory.get_stack(i).await;
                if stack.is_empty() {
                    return true;
                }
            }
            false
        })
        .await
    }

    async fn find_nearest<F, C, Fut>(
        world: &World,
        origin: BlockPos,
        block_matches: F,
        candidate_ok: C,
    ) -> Option<BlockPos>
    where
        F: Fn(&World, &BlockPos) -> bool,
        C: Fn(Arc<dyn crate::block::entities::BlockEntity>) -> Fut,
        Fut: Future<Output = bool>,
    {
        let origin_chunk = origin.chunk_position();
        let mut best: Option<(BlockPos, i32)> = None;

        for dx in -CHUNK_RADIUS..=CHUNK_RADIUS {
            for dz in -CHUNK_RADIUS..=CHUNK_RADIUS {
                let chunk = Vector2::new(origin_chunk.x + dx, origin_chunk.y + dz);
                let Some(entries) = world.block_entities.get(&chunk) else {
                    continue;
                };
                for (pos, block_entity) in entries.iter() {
                    let dist_h = (pos.0.x - origin.0.x)
                        .abs()
                        .max((pos.0.z - origin.0.z).abs());
                    let dist_v = (pos.0.y - origin.0.y).abs();
                    if dist_h > HORIZONTAL_RADIUS || dist_v > VERTICAL_RADIUS {
                        continue;
                    }
                    if !block_matches(world, pos) {
                        continue;
                    }
                    if !candidate_ok(block_entity.clone()).await {
                        continue;
                    }
                    let score = dist_h + dist_v;
                    if best.is_none_or(|(_, best_score)| score < best_score) {
                        best = Some((*pos, score));
                    }
                }
            }
        }

        best.map(|(pos, _)| pos)
    }

    /// The vanilla transport interaction keeps the selected chest active while it reads or
    /// writes the inventory (`CopperGolem.java:413-427`); callers provide that active inventory.
    async fn take_one_item(inventory: &dyn Inventory) -> Option<ItemStack> {
        for i in 0..inventory.size() {
            let stack = inventory.get_stack(i).await;
            if !stack.is_empty() {
                return Some(inventory.remove_stack_specific(i, 1).await);
            }
        }
        None
    }

    async fn deposit_item(inventory: &dyn Inventory, item: ItemStack) -> Option<ItemStack> {
        let mut remaining = item;
        for i in 0..inventory.size() {
            let mut stack = inventory.get_stack(i).await;
            if stack.is_empty() {
                inventory.set_stack(i, remaining).await;
                return None;
            }
            if stack.are_items_and_components_equal(&remaining)
                && stack.item_count < stack.get_max_stack_size()
            {
                let space = stack.get_max_stack_size() - stack.item_count;
                let moved = space.min(remaining.item_count);
                stack.item_count += moved;
                inventory.set_stack(i, stack).await;
                remaining.item_count -= moved;
                if remaining.is_empty() {
                    return None;
                }
            }
        }
        Some(remaining)
    }

    fn copper_golem(mob: &dyn Mob) -> Option<&CopperGolemEntity> {
        mob.cast_any().downcast_ref::<CopperGolemEntity>()
    }

    /// Vanilla `CopperGolem.hasContainerOpen`/`ContainerOpenersCounter` opens a container while
    /// the transport behavior works (`CopperGolem.java:413-423`). The inventory trait already
    /// exposes the matching viewer hooks, so retain the active chest position here as well.
    async fn open_container(
        mob: &dyn Mob,
        world: &World,
        pos: BlockPos,
        state: CopperGolemState,
    ) -> Option<Arc<dyn Inventory>> {
        let block_entity = world.get_block_entity(&pos)?;
        let inventory = block_entity.get_inventory()?;
        if let Some(copper_golem) = Self::copper_golem(mob) {
            copper_golem.set_opened_chest_pos(pos);
            copper_golem.set_state(state);
        }
        inventory.on_open().await;
        Some(inventory)
    }

    /// Vanilla closes the tracked container after the interaction window; the inventory viewer
    /// hook emits the existing chest close event (`CopperGolem.java:413-423`).
    async fn close_container(mob: &dyn Mob, inventory: &dyn Inventory) {
        inventory.on_close().await;
        if let Some(copper_golem) = Self::copper_golem(mob) {
            copper_golem.clear_opened_chest_pos();
            copper_golem.set_state(CopperGolemState::Idle);
        }
    }

    fn block_pos_of(mob: &dyn Mob) -> BlockPos {
        mob.get_mob_entity().living_entity.entity.block_pos.load()
    }

    fn walk_to(mob: &dyn Mob, target: BlockPos, speed: f64) {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let dest = Vector3::new(
            f64::from(target.0.x) + 0.5,
            f64::from(target.0.y),
            f64::from(target.0.z) + 0.5,
        );
        let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
        navigator.set_progress(NavigatorGoal::new(pos, dest, speed));
    }

    fn navigator_idle(mob: &dyn Mob) -> bool {
        mob.get_mob_entity().navigator.lock().unwrap().is_idle()
    }
}

impl Goal for TransportItemsGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.cooldown > 0 {
                self.cooldown -= 1;
                return false;
            }

            let entity = &mob.get_mob_entity().living_entity.entity;
            let world = entity.world.load();
            let origin = Self::block_pos_of(mob);

            let Some(source) = Self::find_source(&world, origin).await else {
                self.cooldown = mob.get_random().random_range(60..=100);
                return false;
            };
            if Self::find_dest(&world, origin).await.is_none() {
                self.cooldown = mob.get_random().random_range(60..=100);
                return false;
            }

            self.phase = Phase::ToSource(source);
            true
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { !matches!(self.phase, Phase::Idle) })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Phase::ToSource(source) = self.phase {
                Self::walk_to(mob, source, self.speed);
            }
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if !Self::navigator_idle(mob) {
                return;
            }

            let entity = &mob.get_mob_entity().living_entity.entity;
            let world = entity.world.load();

            match self.phase {
                Phase::ToSource(source) => {
                    let origin = Self::block_pos_of(mob);
                    // Vanilla rejects arrivals outside `getContainerInteractionRange`
                    // (`CopperGolem.java:425-428`).
                    let interaction_range = Self::copper_golem(mob)
                        .map_or(3.0, CopperGolemEntity::get_container_interaction_range);
                    if f64::from(source.squared_distance(&origin))
                        > interaction_range * interaction_range
                    {
                        self.phase = Phase::Idle;
                        self.cooldown = mob.get_random().random_range(60..=100);
                        return;
                    }
                    // Vanilla selects the corresponding getting animation state
                    // (`CopperGolem.java:331-346`) while the container is queried.
                    let Some(source_inventory) =
                        Self::open_container(mob, &world, source, CopperGolemState::GettingItem)
                            .await
                    else {
                        self.phase = Phase::Idle;
                        self.cooldown = mob.get_random().random_range(60..=100);
                        return;
                    };
                    let item = Self::take_one_item(source_inventory.as_ref()).await;
                    if item.is_none()
                        && let Some(copper_golem) = Self::copper_golem(mob)
                    {
                        copper_golem.set_state(CopperGolemState::GettingNoItem);
                    }
                    Self::close_container(mob, source_inventory.as_ref()).await;
                    let Some(item) = item else {
                        self.phase = Phase::Idle;
                        self.cooldown = mob.get_random().random_range(60..=100);
                        return;
                    };
                    let Some(dest) = Self::find_dest(&world, origin).await else {
                        if let Some(source_inventory) = Self::open_container(
                            mob,
                            &world,
                            source,
                            CopperGolemState::DroppingItem,
                        )
                        .await
                        {
                            let _ = Self::deposit_item(source_inventory.as_ref(), item).await;
                            Self::close_container(mob, source_inventory.as_ref()).await;
                        }
                        self.phase = Phase::Idle;
                        return;
                    };
                    self.carried = Some(item);
                    self.phase = Phase::ToDest { source, dest };
                    Self::walk_to(mob, dest, self.speed);
                }
                Phase::ToDest { source, dest } => {
                    // Vanilla selects the corresponding dropping animation state
                    // (`CopperGolem.java:347-361`) while the destination is queried.
                    if let Some(item) = self.carried.take()
                        && let Some(destination_inventory) =
                            Self::open_container(mob, &world, dest, CopperGolemState::DroppingItem)
                                .await
                    {
                        let leftover =
                            Self::deposit_item(destination_inventory.as_ref(), item).await;
                        if leftover.is_some()
                            && let Some(copper_golem) = Self::copper_golem(mob)
                        {
                            copper_golem.set_state(CopperGolemState::DroppingNoItem);
                        }
                        Self::close_container(mob, destination_inventory.as_ref()).await;
                        if let Some(leftover) = leftover
                            && let Some(source_inventory) = Self::open_container(
                                mob,
                                &world,
                                source,
                                CopperGolemState::DroppingItem,
                            )
                            .await
                        {
                            let _ = Self::deposit_item(source_inventory.as_ref(), leftover).await;
                            Self::close_container(mob, source_inventory.as_ref()).await;
                        }
                    }
                    self.phase = Phase::Idle;
                    self.cooldown = mob.get_random().random_range(60..=100);
                }
                Phase::Idle => {}
            }
        })
    }

    fn stop<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.phase = Phase::Idle;
            self.carried = None;
        })
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
