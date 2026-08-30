// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::{
    BlockProperties, ChestLikeProperties, ChestType, CopperGolemPose,
    CopperGolemStatueLikeProperties,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use uuid::Uuid;

use crate::block::entities::copper_golem_statue::CopperGolemStatueBlockEntity;
use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        interact_with_door::InteractWithDoorGoal, look_at_entity::LookAtEntityGoal,
        transport_items::TransportItemsGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};
use crate::world::World;

/// Copper oxidation stage. Mirrors `WeatheringCopper.WeatherState` (`UNAFFECTED`, `EXPOSED`,
/// `WEATHERED`, `OXIDIZED`) as consulted by `CopperGolem`/`CopperGolemOxidationLevels`.
///
/// This is intentionally a separate type from `copper_weathering.rs`'s block-oxidation
/// helpers: the block side rolls a per-random-tick neighbor-weighted chance
/// (`try_oxidize_copper`), while the mob advances on a fixed game-time schedule
/// (`CopperGolem.updateWeathering`) with no neighbor lookup at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopperWeatherState {
    Unaffected,
    Exposed,
    Weathered,
    Oxidized,
}

impl CopperWeatherState {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Unaffected => Self::Exposed,
            Self::Exposed => Self::Weathered,
            Self::Weathered | Self::Oxidized => Self::Oxidized,
        }
    }

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Unaffected | Self::Exposed => Self::Unaffected,
            Self::Weathered => Self::Exposed,
            Self::Oxidized => Self::Weathered,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unaffected => "unaffected",
            Self::Exposed => "exposed",
            Self::Weathered => "weathered",
            Self::Oxidized => "oxidized",
        }
    }

    /// Wire ordinal of `WeatheringCopper.WeatherState`, as sent in `DATA_WEATHER_STATE`.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Unaffected => 0,
            Self::Exposed => 1,
            Self::Weathered => 2,
            Self::Oxidized => 3,
        }
    }

    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Exposed,
            2 => Self::Weathered,
            3 => Self::Oxidized,
            _ => Self::Unaffected,
        }
    }

    #[must_use]
    pub fn from_name(s: &str) -> Self {
        match s {
            "exposed" => Self::Exposed,
            "weathered" => Self::Weathered,
            "oxidized" => Self::Oxidized,
            _ => Self::Unaffected,
        }
    }

    /// The hurt-sound column of vanilla's per-stage `CopperGolemOxidationLevel` record
    /// (`CopperGolemOxidationLevel.java:6-8`), resolved through the hardcoded table
    /// `CopperGolemOxidationLevels.getOxidationLevel`
    /// (`CopperGolemOxidationLevels.java:9-54`). Unaffected and exposed share the base
    /// sounds (`CopperGolemOxidationLevels.java:9-24`); weathered and oxidized swap in
    /// their own sets (`CopperGolemOxidationLevels.java:25-40`). Consumed server-side by
    /// `CopperGolem.getHurtSound` (`CopperGolem.java:389-391`), which Pumpkin reaches via
    /// the [`Mob::get_hurt_sound`] override below.
    #[must_use]
    pub(crate) const fn oxidation_level_hurt_sound(self) -> Sound {
        match self {
            Self::Weathered => Sound::EntityCopperGolemWeatheredHurt,
            Self::Oxidized => Sound::EntityCopperGolemOxidizedHurt,
            // Unaffected and Exposed both use the base set.
            Self::Unaffected | Self::Exposed => Sound::EntityCopperGolemHurt,
        }
    }

    /// The step-sound column of the same table, consumed server-side by
    /// `CopperGolem.playStepSound` (`CopperGolem.java:399-401`), which Pumpkin reaches via
    /// the [`Mob::get_step_sound`] override below.
    #[must_use]
    pub(crate) const fn oxidation_level_step_sound(self) -> Sound {
        match self {
            Self::Weathered => Sound::EntityCopperGolemWeatheredStep,
            Self::Oxidized => Sound::EntityCopperGolemOxidizedStep,
            Self::Unaffected | Self::Exposed => Sound::EntityCopperGolemStep,
        }
    }
}

/// `CopperGolem.CopperGolemState`, the synched animation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum CopperGolemState {
    #[default]
    Idle = 0,
    GettingItem = 1,
    GettingNoItem = 2,
    DroppingItem = 3,
    DroppingNoItem = 4,
}

impl CopperGolemState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::GettingItem,
            2 => Self::GettingNoItem,
            3 => Self::DroppingItem,
            4 => Self::DroppingNoItem,
            _ => Self::Idle,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// `CopperGolem.UNSET_WEATHERING_TICK`: no schedule set yet, roll one on the next tick.
const UNSET_WEATHERING_TICK: i64 = -1;
/// `CopperGolem.IGNORE_WEATHERING_TICK`: waxed, weathering is frozen entirely.
const IGNORE_WEATHERING_TICK: i64 = -2;
/// `CopperGolem.WEATHERING_TICK_FROM` / `_TO`: ticks between weathering stage advances.
const WEATHERING_TICK_FROM: i64 = 504_000;
const WEATHERING_TICK_TO: i64 = 552_000;
/// `CopperGolem.TURN_TO_STATUE_CHANCE`: rolled once per tick while fully oxidized.
const TURN_TO_STATUE_CHANCE: f32 = 0.0058;

pub struct CopperGolemEntity {
    pub mob_entity: MobEntity,
    weather_state: AtomicCell<CopperWeatherState>,
    state: AtomicI32,
    next_weathering_tick: AtomicCell<i64>,
    last_lightning_bolt_uuid: AtomicCell<Option<Uuid>>,
    /// Vanilla `CopperGolem.openedChestPos`, used by the container viewer query.
    opened_chest_pos: AtomicCell<Option<BlockPos>>,
}

impl CopperGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let golem = Self {
            mob_entity,
            weather_state: AtomicCell::new(CopperWeatherState::Unaffected),
            state: AtomicI32::new(CopperGolemState::Idle.id()),
            next_weathering_tick: AtomicCell::new(UNSET_WEATHERING_TICK),
            last_lightning_bolt_uuid: AtomicCell::new(None),
            opened_chest_pos: AtomicCell::new(None),
        };
        let mob_arc = Arc::new(golem);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        // Vanilla `CopperGolem` constructor: `this.getNavigation().setCanOpenDoors(true);`.
        mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap()
            .set_can_open_doors(true);

        // Cites `CopperGolemAi.initCoreActivity`/`initIdleActivity`. Deferred: `AnimalPanic`
        // (no per-mob danger-flee goal ported here), `SetEntityLookTargetSometimes`,
        // `CountDownCooldownTicks` bookkeeping -- none change observable behavior beyond
        // what `LookAtEntityGoal`/`WanderAroundGoal` already provide.
        #[expect(
            clippy::semicolon_outside_block,
            reason = "conflicts with semicolon_if_nothing_returned for a bare block statement"
        )]
        {
            let mut goal_selector = mob_arc.mob_entity.goals_selector.lock().unwrap();
            goal_selector.add_goal(1, Box::new(InteractWithDoorGoal::new(true)));
            goal_selector.add_goal(2, Box::new(TransportItemsGoal::new(1.0)));
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(1.0)));
        }

        mob_arc
    }

    #[must_use]
    pub fn weather_state(&self) -> CopperWeatherState {
        self.weather_state.load()
    }

    pub(crate) fn set_weather_state(&self, state: CopperWeatherState) {
        self.weather_state.store(state);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::copper_golem::WEATHER_STATE,
                VarInt(state.id()),
            )],
            None,
        );
    }

    #[must_use]
    pub fn get_state(&self) -> CopperGolemState {
        CopperGolemState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: CopperGolemState) {
        self.state.store(state.id(), Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::copper_golem::COPPER_GOLEM_STATE,
                VarInt(state.id()),
            )],
            None,
        );
    }

    /// Vanilla `CopperGolem.setOpenedChestPos`/`clearOpenedChestPos`
    /// (`CopperGolem.java:121-127`) tracks the chest currently being handled.
    pub fn set_opened_chest_pos(&self, opened_chest_pos: BlockPos) {
        self.opened_chest_pos.store(Some(opened_chest_pos));
    }

    pub fn clear_opened_chest_pos(&self) {
        self.opened_chest_pos.store(None);
    }

    /// Vanilla `CopperGolem.hasContainerOpen` (`CopperGolem.java:413-423`) also accepts the
    /// connected half of a double chest. Pumpkin's chest viewer tracker supplies the same
    /// inventory-open/close callbacks for the transport goal.
    #[must_use]
    pub fn has_container_open(&self, block_pos: &BlockPos) -> bool {
        let Some(opened_chest_pos) = self.opened_chest_pos.load() else {
            return false;
        };
        if opened_chest_pos == *block_pos {
            return true;
        }

        let world = self.mob_entity.living_entity.entity.world.load();
        let block = world.get_block(&opened_chest_pos);
        let properties =
            ChestLikeProperties::from_state_id(world.get_block_state_id(&opened_chest_pos), block);
        let connected_direction = match properties.r#type {
            ChestType::Single => return false,
            ChestType::Left => properties.facing.rotate_clockwise(),
            ChestType::Right => properties.facing.rotate_counter_clockwise(),
        };
        opened_chest_pos.offset(connected_direction.to_block_direction().to_offset()) == *block_pos
    }

    /// Vanilla `CopperGolem.getContainerInteractionRange` (`CopperGolem.java:425-428`).
    #[must_use]
    pub const fn get_container_interaction_range(&self) -> f64 {
        3.0
    }

    /// `CopperGolem.updateWeathering`: advances the oxidation stage on a fixed game-time
    /// schedule, then, once fully oxidized, rolls `canTurnToStatue` every tick.
    async fn update_weathering(&self, world: &Arc<World>) {
        let next_tick = self.next_weathering_tick.load();
        if next_tick == IGNORE_WEATHERING_TICK {
            return;
        }

        let state = self.weather_state();
        let is_fully_oxidized = state == CopperWeatherState::Oxidized;

        if next_tick == UNSET_WEATHERING_TICK {
            let game_time = world.get_world_age().await;
            let delay = rand::rng().random_range(WEATHERING_TICK_FROM..=WEATHERING_TICK_TO);
            self.next_weathering_tick.store(game_time + delay);
        } else if !is_fully_oxidized {
            let game_time = world.get_world_age().await;
            if game_time >= next_tick {
                let new_state = state.next();
                self.set_weather_state(new_state);
                if new_state == CopperWeatherState::Oxidized {
                    self.next_weathering_tick.store(0);
                } else {
                    let delay = rand::rng().random_range(WEATHERING_TICK_FROM..=WEATHERING_TICK_TO);
                    self.next_weathering_tick.store(next_tick + delay);
                }
            }
        }

        if is_fully_oxidized && Self::can_turn_to_statue(world, &self.position()) {
            self.turn_to_statue(world).await;
        }
    }

    fn position(&self) -> BlockPos {
        self.mob_entity.living_entity.entity.block_pos.load()
    }

    fn can_turn_to_statue(world: &World, pos: &BlockPos) -> bool {
        let state_id = world.get_block_state_id(pos);
        pumpkin_data::block_properties::is_air(state_id)
            && rand::rng().random::<f32>() <= TURN_TO_STATUE_CHANCE
    }

    /// `CopperGolem.turnToStatue`: replaces the golem with an oxidized copper golem statue
    /// block, always at the fully-oxidized stage (only reachable once the golem itself is
    /// fully oxidized).
    async fn turn_to_statue(&self, world: &Arc<World>) {
        let pos = self.position();
        let entity = &self.mob_entity.living_entity.entity;
        let facing = entity.get_horizontal_facing();

        let mut props =
            CopperGolemStatueLikeProperties::default(&Block::OXIDIZED_COPPER_GOLEM_STATUE);
        props.r#copper_golem_pose = match rand::rng().random_range(0..4) {
            0 => CopperGolemPose::Standing,
            1 => CopperGolemPose::Sitting,
            2 => CopperGolemPose::Running,
            _ => CopperGolemPose::Star,
        };
        props.r#facing = facing;
        let state_id = props.to_state_id(&Block::OXIDIZED_COPPER_GOLEM_STATUE);

        world
            .set_block_state(&pos, state_id, BlockFlags::NOTIFY_ALL)
            .await;

        if let Some(block_entity) = world.get_block_entity(&pos)
            && let Some(statue) = block_entity
                .as_any()
                .downcast_ref::<CopperGolemStatueBlockEntity>()
        {
            statue.create_statue();
            // Vanilla `CopperGolem.turnToStatue` drops preserved equipment after creating the
            // statue and before discarding the entity (`CopperGolem.java:286-301`).
            self.drop_preserved_equipment().await;
        }

        entity.remove().await;
    }

    /// `CopperGolem.mobInteract`: honeycomb waxes (freezes weathering), axe on a waxed golem
    /// un-waxes it, axe on a weathered-but-unwaxed golem scrapes back one stage.
    ///
    fn play_at_self(&self, sound: Sound) {
        let entity = &self.mob_entity.living_entity.entity;
        entity
            .world
            .load()
            .play_sound(sound, SoundCategory::Blocks, &entity.pos.load());
    }

    pub fn golem_interact(&self, player: &Player, item_stack: &mut ItemStack) -> bool {
        if item_stack.item.id == Item::HONEYCOMB.id
            && self.next_weathering_tick.load() != IGNORE_WEATHERING_TICK
        {
            self.next_weathering_tick.store(IGNORE_WEATHERING_TICK);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.play_at_self(Sound::ItemHoneycombWaxOn);
            return true;
        }

        if item_stack.item.has_tag(&tag::Item::MINECRAFT_AXES) {
            if self.next_weathering_tick.load() == IGNORE_WEATHERING_TICK {
                self.next_weathering_tick.store(UNSET_WEATHERING_TICK);
                let _ = item_stack.damage_item(1);
                self.play_at_self(Sound::ItemAxeScrape);
                return true;
            }

            let state = self.weather_state();
            if state != CopperWeatherState::Unaffected {
                self.next_weathering_tick.store(UNSET_WEATHERING_TICK);
                self.set_weather_state(state.previous());
                let _ = item_stack.damage_item(1);
                self.play_at_self(Sound::ItemAxeScrape);
                return true;
            }
        }

        false
    }
}

impl NBTStorage for CopperGolemEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_long("next_weather_age", self.next_weathering_tick.load());
            nbt.put_string("weather_state", self.weather_state().as_str().to_string());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.next_weathering_tick.store(
                nbt.get_long("next_weather_age")
                    .unwrap_or(UNSET_WEATHERING_TICK),
            );
            self.set_weather_state(CopperWeatherState::from_name(
                nbt.get_string("weather_state").unwrap_or("unaffected"),
            ));
        })
    }
}

impl Mob for CopperGolemEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `CopperGolem.getHurtSound` (`CopperGolem.java:389-391`): the hurt sound follows the
    /// oxidation stage through `CopperGolemOxidationLevels.getOxidationLevel`. The generated
    /// `COPPER_GOLEM.hurt_sound` is `None`, so without this override the mob plays the
    /// generic hurt sound.
    fn get_hurt_sound(&self) -> Option<Sound> {
        Some(self.weather_state().oxidation_level_hurt_sound())
    }

    /// `CopperGolem.playStepSound` (`CopperGolem.java:399-401`): the step sound also
    /// follows the oxidation stage, replacing the generic block-step path.
    fn get_step_sound(&self) -> Option<Sound> {
        Some(self.weather_state().oxidation_level_step_sound())
    }

    /// `CopperGolem.thunderHit`: a lightning strike scrubs the golem back to unaffected.
    fn mob_on_lightning_strike<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        lightning: &'a crate::entity::lightning::LightningBoltEntity,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity
                .living_entity
                .on_lightning_strike(caller, lightning)
                .await;

            // `CopperGolem.thunderHit` ignores repeated callbacks for the same bolt and
            // advances one oxidation stage toward unaffected, rather than clearing all stages.
            // The compare-exchange keeps that read/modify/write atomic if lightning dispatch
            // ever reaches this entity concurrently.
            let lightning_uuid = lightning.get_entity().entity_uuid;
            loop {
                let previous = self.last_lightning_bolt_uuid.load();
                if previous == Some(lightning_uuid) {
                    return;
                }
                if self
                    .last_lightning_bolt_uuid
                    .compare_exchange(previous, Some(lightning_uuid))
                    .is_ok()
                {
                    break;
                }
            }

            let state = self.weather_state();
            if state != CopperWeatherState::Unaffected {
                self.next_weathering_tick.store(UNSET_WEATHERING_TICK);
                self.set_weather_state(state.previous());
            }
        })
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.entity.send_meta_data(
                &[
                    Metadata::new(
                        pumpkin_data::tracked_data::copper_golem::WEATHER_STATE,
                        VarInt(self.weather_state().id()),
                    ),
                    Metadata::new(
                        pumpkin_data::tracked_data::copper_golem::COPPER_GOLEM_STATE,
                        VarInt(self.get_state().id()),
                    ),
                ],
                None,
            );
        })
    }

    /// `CopperGolem.actuallyHurt` (CopperGolem.java:450-453): once damage is applied, the
    /// synched animation state snaps back to IDLE, cancelling any in-progress chest
    /// interaction animation. `Mob::on_damage` only runs after the damage landed, which is
    /// exactly vanilla's `actuallyHurt` position in the hurt pipeline.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        _source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.set_state(CopperGolemState::Idle);
        })
    }

    fn mob_tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let world = self.mob_entity.living_entity.entity.world.load();
            self.update_weathering(&world).await;
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if self.golem_interact(player, item_stack) {
                return true;
            }
            self.get_mob_entity()
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CopperWeatherState, IGNORE_WEATHERING_TICK, Sound, UNSET_WEATHERING_TICK,
        WEATHERING_TICK_FROM, WEATHERING_TICK_TO,
    };

    #[test]
    fn weather_state_next_saturates_at_oxidized() {
        assert_eq!(
            CopperWeatherState::Unaffected.next(),
            CopperWeatherState::Exposed
        );
        assert_eq!(
            CopperWeatherState::Exposed.next(),
            CopperWeatherState::Weathered
        );
        assert_eq!(
            CopperWeatherState::Weathered.next(),
            CopperWeatherState::Oxidized
        );
        assert_eq!(
            CopperWeatherState::Oxidized.next(),
            CopperWeatherState::Oxidized
        );
    }

    #[test]
    fn weather_state_previous_saturates_at_unaffected() {
        assert_eq!(
            CopperWeatherState::Oxidized.previous(),
            CopperWeatherState::Weathered
        );
        assert_eq!(
            CopperWeatherState::Weathered.previous(),
            CopperWeatherState::Exposed
        );
        assert_eq!(
            CopperWeatherState::Exposed.previous(),
            CopperWeatherState::Unaffected
        );
        assert_eq!(
            CopperWeatherState::Unaffected.previous(),
            CopperWeatherState::Unaffected
        );
    }

    #[test]
    fn weather_state_name_round_trips() {
        for state in [
            CopperWeatherState::Unaffected,
            CopperWeatherState::Exposed,
            CopperWeatherState::Weathered,
            CopperWeatherState::Oxidized,
        ] {
            assert_eq!(CopperWeatherState::from_name(state.as_str()), state);
        }
    }

    #[test]
    fn weathering_sentinels_are_distinct_from_the_real_schedule_range() {
        const {
            assert!(UNSET_WEATHERING_TICK < 0);
            assert!(IGNORE_WEATHERING_TICK < 0);
            assert!(WEATHERING_TICK_FROM < WEATHERING_TICK_TO);
            assert!(WEATHERING_TICK_FROM > 0);
        }
        assert_ne!(UNSET_WEATHERING_TICK, IGNORE_WEATHERING_TICK);
    }

    #[test]
    fn oxidation_level_sounds_match_the_vanilla_table() {
        // CopperGolemOxidationLevels.java:9-24: unaffected and exposed share the base set.
        assert_eq!(
            CopperWeatherState::Unaffected.oxidation_level_hurt_sound(),
            Sound::EntityCopperGolemHurt
        );
        assert_eq!(
            CopperWeatherState::Exposed.oxidation_level_hurt_sound(),
            Sound::EntityCopperGolemHurt
        );
        assert_eq!(
            CopperWeatherState::Unaffected.oxidation_level_step_sound(),
            Sound::EntityCopperGolemStep
        );
        assert_eq!(
            CopperWeatherState::Exposed.oxidation_level_step_sound(),
            Sound::EntityCopperGolemStep
        );
        // CopperGolemOxidationLevels.java:25-40: weathered and oxidized swap sets.
        assert_eq!(
            CopperWeatherState::Weathered.oxidation_level_hurt_sound(),
            Sound::EntityCopperGolemWeatheredHurt
        );
        assert_eq!(
            CopperWeatherState::Oxidized.oxidation_level_hurt_sound(),
            Sound::EntityCopperGolemOxidizedHurt
        );
        assert_eq!(
            CopperWeatherState::Weathered.oxidation_level_step_sound(),
            Sound::EntityCopperGolemWeatheredStep
        );
        assert_eq!(
            CopperWeatherState::Oxidized.oxidation_level_step_sound(),
            Sound::EntityCopperGolemOxidizedStep
        );
    }
}
