// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering, Ordering::Relaxed};

use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ai::goal::{
        escape_danger::EscapeDangerGoal,
        trader_llama_defend_wandering_trader::TraderLlamaDefendWanderingTraderGoal,
    },
    mob::{Mob, MobEntity, RangedAttackMob},
    passive::{
        animal::Animal,
        equine::{AbstractChestedHorse, AbstractHorse, AbstractHorseData, ChestedHorseData},
        llama::{LlamaData, LlamaMob, register_llama_goals, set_random_strength},
        wandering_trader::WanderingTraderEntity,
    },
    player::Player,
    projectile::llama_spit::LlamaSpitEntity,
};

/// Vanilla `TraderLlama::despawnDelay` default (`TraderLlama.java:28-29`).
const DEFAULT_DESPAWN_DELAY: i32 = 47999;

/// `TraderLlama.java`, layered on the same `LlamaMob`/`AbstractChestedHorse`/`AbstractHorse`
/// stack `LlamaEntity` uses (vanilla `TraderLlama extends Llama`).
///
/// `doPlayerRide`'s mount-block override (`TraderLlama.java:75-81`) is not implemented: Pumpkin
/// has no generic animal-mounting/riding system to attach it to. `finalizeSpawn`'s spawn-reason
/// age-forcing (`TraderLlama.java:117-130`) is also not ported (no `EntitySpawnReason` concept
/// confirmed in this pass).
pub struct TraderLlamaEntity {
    pub mob_entity: MobEntity,
    pub horse_data: AbstractHorseData,
    pub chested_data: ChestedHorseData,
    pub llama_data: LlamaData,
    /// Vanilla `despawnDelay` (`TraderLlama.java:28-29, 71-73`).
    pub despawn_delay: AtomicI32,
}

impl TraderLlamaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let llama = Self {
            mob_entity,
            horse_data: AbstractHorseData::default(),
            chested_data: ChestedHorseData::default(),
            llama_data: LlamaData::default(),
            despawn_delay: AtomicI32::new(DEFAULT_DESPAWN_DELAY),
        };
        let mob_arc = Arc::new(llama);
        AbstractHorse::randomize_attributes(mob_arc.as_ref(), &mut rand::rng());
        set_random_strength(&mob_arc.llama_data, &mut rand::rng());
        mob_arc
            .llama_data
            .variant
            .store(rand::random_range(0..4), Relaxed);

        let dyn_mob: Arc<dyn Mob> = mob_arc.clone();
        let mob_weak = Arc::downgrade(&dyn_mob);
        let llama_dyn: Arc<dyn LlamaMob> = mob_arc.clone();
        let llama_weak = Arc::downgrade(&llama_dyn);
        register_llama_goals(&dyn_mob, mob_weak, llama_weak);

        // `TraderLlama.registerGoals` (`TraderLlama.java:63-68`): an extra, faster
        // `PanicGoal` copy on top of the one `Llama.registerGoals` already adds, plus the
        // wandering-trader-defense target goal. The zombie/`AbstractIllager`
        // `NearestAttackableTargetGoal`s from the same method target a multi-type class
        // predicate that doesn't fit `ActiveTargetGoal`'s single-`EntityType` shape and are
        // out of scope here.
        mob_arc
            .mob_entity
            .goals_selector
            .lock()
            .unwrap()
            .add_goal(1, EscapeDangerGoal::new(2.0));
        mob_arc
            .mob_entity
            .target_selector
            .lock()
            .unwrap()
            .add_goal(1, TraderLlamaDefendWanderingTraderGoal::new());

        mob_arc
    }

    /// `Llama.spit` (`Llama.java:340-365`), also reachable through [`RangedAttackMob`].
    pub fn spit(&self, target: &Arc<dyn EntityBase>) {
        let entity = self.get_entity();
        let world = entity.world.load();

        let spit_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::LLAMA_SPIT);
        let spit = LlamaSpitEntity::new_shot(spit_entity, entity);

        let mob_pos = entity.pos.load();
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let target_height = f64::from(target_entity.entity_dimension.load().height);

        let dx = target_pos.x - mob_pos.x;
        let dy = (target_pos.y + target_height / 3.0) - spit.get_entity().pos.load().y;
        let dz = target_pos.z - mob_pos.z;
        let horizontal_distance = dx.hypot(dz);
        let yo = horizontal_distance * 0.2;

        spit.thrown.set_velocity(dx, dy + yo, dz, 1.5, 10.0);

        let spit_arc: Arc<dyn EntityBase> = Arc::new(spit);
        world.spawn_entity(spit_arc);

        if !entity.silent.load(Ordering::Relaxed) {
            world.play_sound(Sound::EntityLlamaSpit, SoundCategory::Neutral, &mob_pos);
        }
        // `Llama.spit` (`Llama.java:363`).
        self.llama_data.did_spit.store(true, Relaxed);
    }
}

impl NBTStorage for TraderLlamaEntity {
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.mob_entity.living_entity.write_nbt(nbt);
        self.write_animal_nbt(nbt);
        self.write_horse_nbt(nbt);
        self.write_chested_horse_nbt(nbt);
        self.write_llama_nbt(nbt);
        nbt.put_int("DespawnDelay", self.despawn_delay.load(Ordering::Relaxed));
    }

    fn read_nbt_non_mut(&self, nbt: &NbtCompound) {
        self.mob_entity.living_entity.read_nbt_non_mut(nbt);
        self.read_animal_nbt(nbt);
        self.read_llama_strength_variant(nbt);
        self.read_horse_nbt(nbt);
        self.read_chested_horse_nbt(nbt);
        if let Some(delay) = nbt.get_int("DespawnDelay") {
            self.despawn_delay.store(delay, Ordering::Relaxed);
        }
    }
}

impl Animal for TraderLlamaEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_LLAMA_FOOD)
    }
}

impl AbstractHorse for TraderLlamaEntity {
    fn horse_data(&self) -> &AbstractHorseData {
        &self.horse_data
    }

    /// `AbstractChestedHorse.randomizeAttributes`: only max-health is rolled.
    fn randomize_attributes(&self, random: &mut impl RngExt)
    where
        Self: Sized,
    {
        crate::entity::passive::llama::randomize_llama_max_health(self, random);
    }

    fn max_temper(&self) -> i32 {
        30
    }

    fn can_perform_rearing(&self) -> bool {
        false
    }

    fn angry_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaAngry)
    }

    fn eating_sound(&self) -> Option<Sound> {
        Some(Sound::EntityLlamaEat)
    }

    fn handle_eating(&self, player: &Arc<Player>, item_stack: &ItemStack) -> bool {
        self.handle_llama_eating(player, item_stack)
    }
}

impl AbstractChestedHorse for TraderLlamaEntity {
    fn chested_data(&self) -> &ChestedHorseData {
        &self.chested_data
    }

    fn get_inventory_columns(&self) -> u8 {
        if self.has_chest() {
            self.llama_data.strength.load(Relaxed)
        } else {
            0
        }
    }

    fn play_chest_equips_sound(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityLlamaChest,
            pumpkin_data::sound::SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }
}

impl LlamaMob for TraderLlamaEntity {
    fn llama_data(&self) -> &LlamaData {
        &self.llama_data
    }
}

impl Mob for TraderLlamaEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `ServerPlayer.openHorseInventory` receives the chested horse container
    /// (`ServerPlayer.java:1372-1382`) after the ridden-vehicle inventory command.
    fn open_custom_inventory_screen(&self, player: &Arc<Player>) {
        if self.is_tamed() {
            AbstractChestedHorse::open_chest_inventory(self, player);
        }
    }

    fn get_follow_leash_speed(&self) -> f32 {
        2.0
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.chested_mob_interact(player, item_stack)
    }

    fn mob_init_data_tracker(&self) {
        crate::entity::passive::llama::send_baby_id_if_baby(self.get_entity());
        self.send_llama_metadata();
    }

    fn create_offspring(
        &self,
        mate: &dyn EntityBase,
        world: &Arc<crate::world::World>,
    ) -> Option<Arc<dyn EntityBase>> {
        self.create_llama_offspring(mate, world)
    }

    /// Vanilla `TraderLlama::maybeDespawn` (`TraderLlama.java:91-99`). `canDespawn`
    /// (`TraderLlama.java:101-107`) now checks `is_tamed` (available since `Llama` gained real
    /// `AbstractHorse` taming) but still can't check `hasExactlyOnePlayerPassenger`, `isAgeLocked`
    /// or `isPersistenceRequired` -- none of those concepts exist on any Pumpkin entity yet, a
    /// pre-existing gap in the despawn system generally, not introduced here. While leashed to a
    /// `WanderingTrader`, the countdown slaves itself to the trader's own `despawn_delay` minus
    /// one every tick (`TraderLlama.java:93-95`) so it automatically tracks any reset/extension of
    /// the trader's timer; otherwise it decrements independently.
    fn mob_tick(&self, _caller: &Arc<dyn EntityBase>) {
        self.tick_horse_ai();
        let entity = &self.mob_entity.living_entity.entity;
        let holder = entity
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let leashed_to_other = holder.is_some()
            && holder
                .as_ref()
                .and_then(|h| h.cast_any().downcast_ref::<WanderingTraderEntity>())
                .is_none();

        let can_despawn = !self.is_tamed() && !leashed_to_other;
        if !can_despawn {
            return;
        }

        let trader = holder
            .as_ref()
            .and_then(|h| h.cast_any().downcast_ref::<WanderingTraderEntity>());

        let new_delay = trader.map_or_else(
            || self.despawn_delay.load(Ordering::Relaxed) - 1,
            |trader| trader.despawn_delay.load(Ordering::Relaxed) - 1,
        );
        self.despawn_delay.store(new_delay, Ordering::Relaxed);

        if new_delay <= 0 {
            entity.unleash();
            let world = entity.world.load();
            world.remove_entity(self);
        }
    }
}

impl RangedAttackMob for TraderLlamaEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        self.spit(target);
    }
}
