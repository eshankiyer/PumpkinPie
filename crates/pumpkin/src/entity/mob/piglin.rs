use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal,
        piglin_admire::{ADMIRE_DURATION_TICKS, PiglinAdmireGoal},
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, piglin_shared,
        zombification::{self, ZombificationTimer},
        zombified_piglin::ZombifiedPiglinEntity,
    },
    player::Player,
};
use crate::world::World;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    damage::DamageType, data_component_impl::EquipmentSlot, entity::EntityType, item::Item,
    item_stack::ItemStack, sound::Sound,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;

use crate::entity::ai::target_predicate::TargetData;

/// `PiglinAi.ADMIRING_DISABLED` lockout: `wasHurtBy` (PiglinAi.java:567) sets a
/// 400-tick expiry when the attacker is a player, blocking `canAdmire` for that long.
const ADMIRING_DISABLED_TICKS: i32 = 400;

/// `PiglinAi.isWearingSafeArmor` (PiglinAi.java:648-656): true when ANY of the four armor
/// slots holds an item in the `minecraft:piglin_safe_armor` tag (the four gold pieces).
pub(crate) async fn is_wearing_safe_armor(living: &crate::entity::living::LivingEntity) -> bool {
    const ARMOR_SLOTS: [EquipmentSlot; 4] = [
        EquipmentSlot::HEAD,
        EquipmentSlot::CHEST,
        EquipmentSlot::LEGS,
        EquipmentSlot::FEET,
    ];
    let equipment = living.entity_equipment.lock().await;
    ARMOR_SLOTS.iter().any(|slot| {
        equipment
            .get(slot)
            .item
            .has_tag(&tag::Item::MINECRAFT_PIGLIN_SAFE_ARMOR)
    })
}

/// The `NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD` half of `PiglinSpecificSensor.doTick`
/// (PiglinSpecificSensor.java:82-85), which is the only player memory
/// `PiglinAi.findNearestValidAttackTarget` falls back on for an unprovoked piglin
/// (PiglinAi.java:531-532).
///
/// Anger is deliberately NOT routed through this predicate: vanilla checks the `ANGRY_AT`
/// memory first (PiglinAi.java:515-518), so a piglin that has been hit still retaliates
/// against a fully gold-armoured player. Here that path is `on_damage` ->
/// `piglin_shared::retaliate_and_alert_piglins`, which sets the target directly and never
/// consults this goal.
async fn player_not_wearing_gold(target: TargetData, world: Arc<World>) -> bool {
    let Some(entity) = world.get_entity_by_id(target.entity_id) else {
        return false;
    };
    let Some(living) = entity.get_living_entity() else {
        return false;
    };
    !is_wearing_safe_armor(living).await
}

pub struct PiglinEntity {
    pub mob_entity: MobEntity,
    /// Ticks left staring at a gold ingot given by a player, shared with
    /// `PiglinAdmireGoal`. See `pumpkin/src/entity/ai/goal/piglin_admire.rs`.
    pub admiring_ticks: Arc<AtomicI32>,
    /// Mirrors vanilla's `ADMIRING_DISABLED` memory expiry (see `ADMIRING_DISABLED_TICKS`).
    admiring_disabled_ticks: AtomicI32,
    /// `AbstractPiglin.timeInOverworld`/`IsImmuneToZombification`
    /// (`AbstractPiglin.java:26-33`).
    zombification: ZombificationTimer,
}

impl PiglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let admiring_ticks = Arc::new(AtomicI32::new(0));
        let piglin = Self {
            mob_entity,
            admiring_ticks: admiring_ticks.clone(),
            admiring_disabled_ticks: AtomicI32::new(0),
            zombification: ZombificationTimer::new(),
        };
        let mob_arc = Arc::new(piglin);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, PiglinAdmireGoal::new(admiring_ticks));
            // Piglins use crossbows or swords, but for now we give them melee
            goal_selector.add_goal(2, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(
                1,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(player_not_wearing_gold),
                )),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(
                    &mob_arc.mob_entity,
                    &EntityType::WITHER_SKELETON,
                    true,
                ),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::WITHER, true),
            );
        };

        mob_arc
    }

    fn is_adult(&self) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            >= 0
    }
}

impl NBTStorage for PiglinEntity {
    /// `AbstractPiglin.addAdditionalSaveData` (`AbstractPiglin.java:65-70`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.zombification.write_nbt(nbt);
        })
    }

    /// `AbstractPiglin.readAdditionalSaveData` (`AbstractPiglin.java:72-78`). The
    /// immunity flag is synced because vanilla holds it in `DATA_IMMUNE_TO_ZOMBIFICATION`
    /// (`AbstractPiglin.java:26-28`), which `setImmuneToZombification` writes through.
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.zombification.read_nbt(nbt);
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION,
                    self.zombification.is_immune(),
                )],
                None,
            );
        })
    }
}

impl Mob for PiglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `PiglinAi.mobInteract`/`canAdmire` (PiglinAi.java:539-554): a player directly
    /// handing an adult, non-admiring, non-locked-out piglin a gold ingot starts
    /// admiring. Ground-item pickup (`isLovedItem`/broader `piglin_loved` tag) is not
    /// implemented -- see `piglin_admire.rs` module doc.
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let is_gold_ingot = item_stack.item.id == Item::GOLD_INGOT.id;
            let admiring_disabled = self.admiring_disabled_ticks.load(Ordering::Relaxed) > 0;
            let already_admiring = self.admiring_ticks.load(Ordering::Relaxed) > 0;

            if !is_gold_ingot || admiring_disabled || already_admiring || !self.is_adult() {
                return self
                    .mob_entity
                    .mob_interact(player, item_stack, self.can_be_leashed())
                    .await;
            }

            let taken = ItemStack::new(1, &Item::GOLD_INGOT);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);

            let mut equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            let previous = equipment.put(&EquipmentSlot::OFF_HAND, taken.clone());
            drop(equipment);
            if !previous.is_empty() {
                // `holdInOffhand` (PiglinAi.java:353-359): drop whatever was already
                // held in the offhand before replacing it.
                let pos = self.mob_entity.living_entity.entity.block_pos.load();
                self.mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .drop_stack(&pos, previous)
                    .await;
            }
            self.mob_entity
                .living_entity
                .send_equipment_changes(&[(EquipmentSlot::OFF_HAND, taken)]);

            self.admiring_ticks
                .store(ADMIRE_DURATION_TICKS, Ordering::Relaxed);
            true
        })
    }

    /// `PiglinAi.wasHurtBy` (PiglinAi.java:556-587), simplified: cancels admiring,
    /// applies the player-attacker lockout, then retaliates and alerts nearby piglins
    /// (see `piglin_shared::retaliate_and_alert_piglins`). Not implemented: baby-flee
    /// (100-tick flee instead of retaliating) and the hoglin-outnumbered retreat
    /// branch -- both need per-mob-type special-casing beyond this goal-based
    /// approximation of `maybeRetaliate`.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if let Some(source) = source
                && source.get_entity().entity_type.id == EntityType::PIGLIN.id
            {
                // vanilla: `if (!(attacker instanceof Piglin))` guards the whole method.
                return;
            }

            self.admiring_ticks.store(0, Ordering::Relaxed);

            // `cancelAdmiring` (PiglinAi.java:401-406): drop the offhand item instead
            // of silently voiding it.
            let mut equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            let dropped = equipment.put(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());
            drop(equipment);
            if !dropped.is_empty() {
                let pos = self.mob_entity.living_entity.entity.block_pos.load();
                self.mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .drop_stack(&pos, dropped)
                    .await;
                self.mob_entity
                    .living_entity
                    .send_equipment_changes(&[(EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone())]);
            }

            let attacker_is_player =
                source.is_some_and(|s| s.get_entity().entity_type.id == EntityType::PLAYER.id);
            if attacker_is_player {
                self.admiring_disabled_ticks
                    .store(ADMIRING_DISABLED_TICKS, Ordering::Relaxed);
            }

            if let Some(source) = source {
                piglin_shared::retaliate_and_alert_piglins(self, source).await;
            }
        })
    }

    /// Decrements the admiring-disabled lockout every tick, matching the passive
    /// expiry of vanilla's `ADMIRING_DISABLED` memory. Must run independently of
    /// `PiglinAdmireGoal` since the goal isn't running while admiring is disabled.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let remaining = self.admiring_disabled_ticks.load(Ordering::Relaxed);
            if remaining > 0 {
                self.admiring_disabled_ticks
                    .store(remaining - 1, Ordering::Relaxed);
            }

            // `AbstractPiglin.customServerAiStep` (`AbstractPiglin.java:80-96`).
            if self.zombification.tick(&self.mob_entity) {
                if self
                    .mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .level_info
                    .load()
                    .difficulty
                    != Difficulty::Peaceful
                {
                    zombification::play_converted_sound(
                        &self.mob_entity,
                        Sound::EntityPiglinConvertedToZombified,
                    );
                }
                zombification::convert_to(
                    &self.mob_entity,
                    &EntityType::ZOMBIFIED_PIGLIN,
                    ZombifiedPiglinEntity::new,
                )
                .await;
            }
        })
    }
}
