use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use crossbeam::atomic::AtomicCell;

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::GameMode;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal, defend_village_target::DefendVillageTargetGoal,
        golem_random_stroll_in_village::GolemRandomStrollInVillageGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        move_back_to_village::MoveBackToVillageGoal,
        move_towards_target::MoveTowardsTargetGoal, offer_flower::OfferFlowerGoal,
        nearest_hostile_target::NearestHostileTargetGoal,
        reset_universal_anger_target::ResetUniversalAngerTargetGoal, revenge::RevengeGoal,
    },
    mob::{Mob, MobEntity},
    persistent_anger::PersistentAnger,
    player::Player,
};
use crate::world::World;

/// `IronGolem.IRON_INGOT_HEAL_AMOUNT` (`IronGolem.java:54`).
const IRON_INGOT_HEAL_AMOUNT: f32 = 25.0;

/// `Crackiness.Level` (`Crackiness.java:36-40`) selected by `Crackiness.byFraction`
/// (`Crackiness.java:6`, `Crackiness.java:18-25`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Crackiness { None, Low, Medium, High }

fn golem_crackiness(health: f32, max_health: f32) -> Crackiness {
    let fraction = health / max_health;
    if fraction < 0.25 { Crackiness::High }
    else if fraction < 0.5 { Crackiness::Medium }
    else if fraction < 0.75 { Crackiness::Low }
    else { Crackiness::None }
}

/// `IronGolem.doHurtTarget` damage roll (`IronGolem.java:192`).
fn attack_damage_roll(base_attack_damage: f32, rand_int: i32) -> f32 {
    if base_attack_damage as i32 > 0 { base_attack_damage / 2.0 + rand_int as f32 } else { base_attack_damage }
}

/// Represents an Iron Golem, a powerful neutral mob that protects villagers and players.
///
/// Wiki: <https://minecraft.wiki/w/Iron_Golem>
pub struct IronGolemEntity {
    pub mob_entity: MobEntity,
    pub player_created: AtomicBool,
    health_before_damage: AtomicCell<f32>,
    pub attack_animation_tick: AtomicI32,
    pub offer_flower_tick: AtomicI32,
    pub persistent_anger: PersistentAnger,
}

impl IronGolemEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let iron_golem = Self {
            mob_entity,
            player_created: AtomicBool::new(false),
            health_before_damage: AtomicCell::new(0.0),
            attack_animation_tick: AtomicI32::new(0),
            offer_flower_tick: AtomicI32::new(0),
            persistent_anger: PersistentAnger::default(),
        };
        let mob_arc = Arc::new(iron_golem);
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
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `IronGolem.registerGoals` (`IronGolem.java:69-71`) uses village stroll/back-to-village
            // goals rather than the generic random stroll.
            goal_selector.add_goal(1, Box::new(MeleeAttackGoal::new(1.0, true)));
            goal_selector.add_goal(2, MoveTowardsTargetGoal::new(0.9, 32.0));
            goal_selector.add_goal(2, MoveBackToVillageGoal::new(0.6, false));
            goal_selector.add_goal(4, GolemRandomStrollInVillageGoal::new(0.6));
            goal_selector.add_goal(5, Box::new(OfferFlowerGoal::new()));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            // `DefendVillageTargetGoal` is vanilla priority 1 (`IronGolem.java:75`).
            target_selector.add_goal(1, DefendVillageTargetGoal::new());
            target_selector.add_goal(2, Box::new(RevengeGoal::new(true)));
            // `IronGolem.java:77`: player targets are gated by the golem's persistent anger.
            let angry_weak = mob_weak.clone();
            target_selector.add_goal(
                3,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(move |target: crate::entity::ai::target_predicate::TargetData, world: Arc<World>| {
                        let angry_weak = angry_weak.clone();
                        async move {
                            let Some(mob) = angry_weak.upgrade() else { return false; };
                            let Some(anger) = mob.persistent_anger() else { return false; };
                            if anger.is_angry_at(target.entity_uuid).await { return true; }
                            anger.is_angry_at_all_players(
                                world.level_info.load().game_rules.universal_anger,
                            ).await
                        }
                    }),
                )),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::ZOMBIE, true),
            );
            target_selector.add_goal(3, NearestHostileTargetGoal::new(&mob_arc.mob_entity));
            target_selector.add_goal(4, ResetUniversalAngerTargetGoal::new(false));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_player_created(&self) -> bool {
        self.player_created.load(Ordering::Relaxed)
    }

    pub fn set_player_created(&self, value: bool) {
        // `IronGolem.DATA_PLAYER_CREATED_ID` (`IronGolem.java:287-291`) is mirrored to clients.
        self.player_created.store(value, Ordering::Relaxed);
        let entity = self.get_entity();
        let flag: u8 = u8::from(value);
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::iron_golem::FLAGS_ID,
                flag,
            )],
            None,
        );
    }

    pub fn offer_flower(&self, offer: bool) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if offer {
            self.offer_flower_tick.store(400, Ordering::Relaxed);
            world.send_entity_status(entity, EntityStatus::OfferFlower, None);
        } else {
            self.offer_flower_tick.store(0, Ordering::Relaxed);
            world.send_entity_status(entity, EntityStatus::StopOfferFlower, None);
        }
    }

    #[must_use]
    pub fn get_offer_flower_tick(&self) -> i32 {
        self.offer_flower_tick.load(Ordering::Relaxed)
    }
}

impl Mob for IronGolemEntity {
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        // `IronGolem.java:147`: player-created state is persistent.
        nbt.put_bool("PlayerCreated", self.is_player_created());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // `IronGolem.java:154`: absent PlayerCreated defaults to false.
        if let Some(created) = nbt.get_bool("PlayerCreated") {
            self.set_player_created(created);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `IronGolem.canAttack` (`IronGolem.java:135-142`; `IronGolem.java:136-141`).
    fn can_attack(&self, target: &Entity) -> bool {
        if self.player_created.load(Ordering::Relaxed) && target.entity_type == &EntityType::PLAYER {
            return false;
        }
        target.entity_type != &EntityType::CREEPER
    }

    fn persistent_anger(&self) -> Option<&PersistentAnger> {
        Some(&self.persistent_anger)
    }

    /// `IronGolem.doHurtTarget` (`IronGolem.java:187-204`) uses a randomized attack roll and
    /// launches the victim upward according to knockback resistance.
    fn try_attack<'a>(&'a self, target: &'a dyn EntityBase) -> crate::entity::EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if self.mob_entity.living_entity.dead.load(Ordering::Relaxed) {
                return false;
            }
            let living = &self.mob_entity.living_entity;
            let entity = &living.entity;
            let world = entity.world.load();
            world.send_entity_status(entity, EntityStatus::StartAttacking, None);
            let base = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
            let roll = if base as i32 > 0 { rand::rng().random_range(0..base as i32) } else { 0 };
            let damage = attack_damage_roll(base, roll);
            let caller = world.get_entity_by_id(entity.entity_id);
            let damaged = target.damage_with_context(
                target, damage, DamageType::MOB_ATTACK, Some(entity.pos.load()),
                caller.as_deref(), caller.as_deref(),
            );
            if damaged {
                let resistance = target.get_living_entity().map_or(0.0, |l| {
                    l.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE)
                });
                let scale = (1.0 - resistance).max(0.0);
                target.get_entity().add_velocity(Vector3::new(0.0, 0.4 * scale, 0.0));
            }
            // `IronGolem.java:202`: the attack sound is emitted even when damage is resisted.
            world.play_sound(Sound::EntityIronGolemAttack, SoundCategory::Neutral, &entity.pos.load());
            damaged
        })
    }

    /// `IronGolem.hurtServer` (`IronGolem.java:206-215`, `IronGolem.java:210-212`) compares crackiness before and after
    /// applying damage.
    fn pre_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) -> bool {
        self.health_before_damage.store(self.mob_entity.living_entity.health.load());
        true
    }

    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        let before = golem_crackiness(self.health_before_damage.load(), living.get_max_health());
        let after = golem_crackiness(living.health.load(), living.get_max_health());
        if before != after {
            living.entity.play_sound(Sound::EntityIronGolemDamage);
        }
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        // `IronGolem.aiStep` (`IronGolem.java:125-127`) also refreshes persistent anger while
        // a target is held; the shared anger predicate above remains active for that state.
        let attack_tick = self.attack_animation_tick.load(Ordering::Relaxed);
        if attack_tick > 0 {
            self.attack_animation_tick.fetch_sub(1, Ordering::Relaxed);
        }

        let flower_tick = self.offer_flower_tick.load(Ordering::Relaxed);
        if flower_tick > 0 {
            self.offer_flower_tick.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let flag: u8 = u8::from(self.is_player_created());
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::iron_golem::FLAGS_ID,
                flag,
            )],
            None,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        // `Mob.checkAndHandleImportantInteractions` runs before `mobInteract`: leash/nametag
        // handling wins over the repair item, as in vanilla.
        // `IronGolem.mobInteract` (`IronGolem.java:259-276`) handles important interactions first.
        if self
            .get_mob_entity()
            .mob_interact_with_leash(player, item_stack, self.can_be_leashed())
        {
            return true;
        }

        if item_stack.item.id == Item::IRON_INGOT.id {
            let living = &self.mob_entity.living_entity;
            let health_before = living.health.load();
            living.heal(IRON_INGOT_HEAL_AMOUNT);
            if living.health.load() == health_before {
                return false;
            }
            let entity = self.get_entity();
            let world = entity.world.load();
            let mut rng = rand::rng();
            let pitch = 1.0 + (rng.random::<f32>() - rng.random::<f32>()) * 0.2;
            world.play_sound_fine(
                Sound::EntityIronGolemRepair,
                SoundCategory::Neutral,
                &entity.pos.load(),
                1.0,
                pitch,
            );
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{Crackiness, attack_damage_roll, golem_crackiness};

    #[test]
    fn crackiness_tiers_match_golem_thresholds() {
        assert_eq!(golem_crackiness(100.0, 100.0), Crackiness::None);
        assert_eq!(golem_crackiness(75.0, 100.0), Crackiness::None);
        assert_eq!(golem_crackiness(74.0, 100.0), Crackiness::Low);
        assert_eq!(golem_crackiness(50.0, 100.0), Crackiness::Low);
        assert_eq!(golem_crackiness(49.0, 100.0), Crackiness::Medium);
        assert_eq!(golem_crackiness(25.0, 100.0), Crackiness::Medium);
        assert_eq!(golem_crackiness(24.0, 100.0), Crackiness::High);
    }

    #[test]
    fn damage_roll_adds_half_base_to_the_roll() {
        assert_eq!(attack_damage_roll(15.0, 0), 7.5);
        assert_eq!(attack_damage_roll(15.0, 14), 21.5);
    }

    #[test]
    fn non_positive_base_damage_is_flat() {
        assert_eq!(attack_damage_roll(0.0, 0), 0.0);
        assert_eq!(attack_damage_roll(0.5, 3), 0.5);
    }
}
