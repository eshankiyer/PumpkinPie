// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Weak;
use std::sync::atomic::Ordering::Relaxed;

use crossbeam::atomic::AtomicCell;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Difficulty;

use crate::entity::ai::goal::spellcaster::{IllagerSpell, SpellCastTimer};
use crate::entity::ai::goal::{Controls, Goal};
use crate::entity::mob::Mob;
use crate::entity::mob::illusioner::IllusionerEntity;

/// Vanilla: `Illusioner.registerGoals`'s `SpellcasterIllager.SpellcasterCastingSpellGoal`. Stops
/// navigation and keeps the illusioner looking at its target while a spell is active.
pub struct IllusionerCastingSpellGoal {
    illusioner: Weak<IllusionerEntity>,
}

impl IllusionerCastingSpellGoal {
    #[must_use]
    pub const fn new(illusioner: Weak<IllusionerEntity>) -> Self {
        Self { illusioner }
    }
}

impl Goal for IllusionerCastingSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.illusioner
            .upgrade()
            .is_some_and(|illusioner| illusioner.spellcaster.is_casting_spell())
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        self.illusioner
            .upgrade()
            .is_some_and(|illusioner| illusioner.spellcaster.is_casting_spell())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        if let Some(illusioner) = self.illusioner.upgrade() {
            illusioner.mob_entity.navigator.lock().unwrap().stop();
        }
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(illusioner) = self.illusioner.upgrade() {
            illusioner.spellcaster.set_current_spell(IllagerSpell::None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let target = illusioner
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(target) = target {
            illusioner
                .mob_entity
                .look_control
                .lock()
                .unwrap()
                .look_at_entity_with_range(
                    &target,
                    mob.get_max_look_yaw_change(),
                    mob.get_max_look_pitch_change(),
                );
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// Vanilla: `SpellcasterIllager.SpellcasterUseSpellGoal#canUse`, minus the spell-specific extra
/// gate each concrete goal adds on top.
fn generic_use_spell_can_use(illusioner: &IllusionerEntity, next_attack_tick_count: i32) -> bool {
    let target = illusioner
        .mob_entity
        .target
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(target) = target else {
        return false;
    };
    if !target.get_entity().is_alive() || illusioner.spellcaster.is_casting_spell() {
        return false;
    }
    illusioner.mob_entity.living_entity.entity.age.load(Relaxed) >= next_attack_tick_count
}

/// Vanilla: `Illusioner.IllusionerMirrorSpellGoal`. Turns the illusioner invisible.
pub struct IllusionerMirrorSpellGoal {
    illusioner: Weak<IllusionerEntity>,
    timer: SpellCastTimer,
}

impl IllusionerMirrorSpellGoal {
    const CASTING_TIME: i32 = 20;
    const CASTING_INTERVAL: i32 = 340;

    #[must_use]
    pub const fn new(illusioner: Weak<IllusionerEntity>) -> Self {
        Self {
            illusioner,
            timer: SpellCastTimer::new(),
        }
    }
}

impl Goal for IllusionerMirrorSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        if !generic_use_spell_can_use(&illusioner, self.timer.next_attack_tick_count) {
            return false;
        }
        illusioner
            .mob_entity
            .living_entity
            .get_effect(&StatusEffect::INVISIBILITY)
            .is_none()
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        let target = illusioner
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        target.is_some_and(|target| target.get_entity().is_alive())
            && self.timer.attack_warmup_delay > 0
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let tick_count = illusioner.mob_entity.living_entity.entity.age.load(Relaxed);
        self.timer.start(
            tick_count,
            20,
            Self::CASTING_TIME,
            Self::CASTING_INTERVAL,
            &illusioner.spellcaster,
            IllagerSpell::Disappear,
        );
        illusioner
            .mob_entity
            .living_entity
            .entity
            .world
            .load()
            .play_sound(
                Sound::EntityIllusionerPrepareMirror,
                SoundCategory::Hostile,
                &illusioner.mob_entity.living_entity.entity.pos.load(),
            );
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        if self.timer.tick() {
            illusioner.mob_entity.living_entity.add_effect(Effect {
                effect_type: &StatusEffect::INVISIBILITY,
                duration: 1200,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            });
            illusioner
                .mob_entity
                .living_entity
                .entity
                .world
                .load()
                .play_sound(
                    Sound::EntityIllusionerCastSpell,
                    SoundCategory::Hostile,
                    &illusioner.mob_entity.living_entity.entity.pos.load(),
                );
        }
    }
}

/// Vanilla: `Illusioner.IllusionerBlindnessSpellGoal`. Blinds the current target, but only once
/// per new target and only on `Hard` local difficulty (`isHarderThan(NORMAL)`).
pub struct IllusionerBlindnessSpellGoal {
    illusioner: Weak<IllusionerEntity>,
    timer: SpellCastTimer,
    last_target_id: AtomicCell<Option<i32>>,
}

impl IllusionerBlindnessSpellGoal {
    const CASTING_TIME: i32 = 20;
    const CASTING_INTERVAL: i32 = 180;

    #[must_use]
    pub const fn new(illusioner: Weak<IllusionerEntity>) -> Self {
        Self {
            illusioner,
            timer: SpellCastTimer::new(),
            last_target_id: AtomicCell::new(None),
        }
    }
}

impl Goal for IllusionerBlindnessSpellGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        if !generic_use_spell_can_use(&illusioner, self.timer.next_attack_tick_count) {
            return false;
        }
        let Some(target) = illusioner
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return false;
        };
        if Some(target.get_entity().entity_id) == self.last_target_id.load() {
            return false;
        }
        let world = illusioner.mob_entity.living_entity.entity.world.load();
        // Vanilla: `DifficultyInstance.isHarderThan(Difficulty.NORMAL.ordinal())`; `Hard` is
        // the only `Difficulty` variant ordered after `Normal`.
        world.level_info.load().difficulty == Difficulty::Hard
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return false;
        };
        let target = illusioner
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        target.is_some_and(|target| target.get_entity().is_alive())
            && self.timer.attack_warmup_delay > 0
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        let tick_count = illusioner.mob_entity.living_entity.entity.age.load(Relaxed);
        self.timer.start(
            tick_count,
            20,
            Self::CASTING_TIME,
            Self::CASTING_INTERVAL,
            &illusioner.spellcaster,
            IllagerSpell::Blindness,
        );
        if let Some(target) = illusioner
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            self.last_target_id
                .store(Some(target.get_entity().entity_id));
        }
        illusioner
            .mob_entity
            .living_entity
            .entity
            .world
            .load()
            .play_sound(
                Sound::EntityIllusionerPrepareBlindness,
                SoundCategory::Hostile,
                &illusioner.mob_entity.living_entity.entity.pos.load(),
            );
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(illusioner) = self.illusioner.upgrade() else {
            return;
        };
        if self.timer.tick() {
            let target = illusioner
                .mob_entity
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(target) = target
                && let Some(living) = target.get_living_entity()
            {
                living.add_effect(Effect {
                    effect_type: &StatusEffect::BLINDNESS,
                    duration: 400,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: false,
                });
            }
            illusioner
                .mob_entity
                .living_entity
                .entity
                .world
                .load()
                .play_sound(
                    Sound::EntityIllusionerCastSpell,
                    SoundCategory::Hostile,
                    &illusioner.mob_entity.living_entity.entity.pos.load(),
                );
        }
    }
}
