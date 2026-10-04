use std::sync::Arc;

use super::{NBTStorage, NBTStorageInit, player::Player};
use crate::entity::NbtFuture;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::Difficulty;

const MAX_FOOD: u8 = 20;
const EXHAUSTION_COST: f32 = 4.0;
const MAX_EXHAUSTION: f32 = 40.0;

/// Vanilla `FoodConstants.saturationByModifier` (`FoodConstants.java:30-32`).
#[must_use]
pub fn saturation_by_modifier(nutrition: i32, modifier: f32) -> f32 {
    nutrition as f32 * modifier * 2.0
}

pub struct HungerManager {
    pub level: AtomicCell<u8>,
    pub saturation: AtomicCell<f32>,
    pub exhaustion: AtomicCell<f32>,
    pub tick_timer: AtomicCell<u32>,
}

impl Default for HungerManager {
    fn default() -> Self {
        Self {
            level: AtomicCell::new(MAX_FOOD),
            saturation: AtomicCell::new(5.0),
            exhaustion: AtomicCell::new(0.0),
            tick_timer: AtomicCell::new(0),
        }
    }
}

impl HungerManager {
    pub async fn tick(&self, player: &Arc<Player>) {
        let mut level = self.level.load();
        let mut saturation = self.saturation.load();
        let mut exhaustion = self.exhaustion.load();
        let mut timer = self.tick_timer.load();

        let level_info = player.world().level_info.load();
        let difficulty = level_info.difficulty;
        let natural_regen = level_info.game_rules.natural_health_regeneration;
        let health = player.living_entity.health.load();
        let can_heal = player.can_food_heal();

        let mut needs_sync = false;
        let mut heal_amount = 0.0;
        let mut damage_amount = 0.0;

        if exhaustion > EXHAUSTION_COST {
            exhaustion -= EXHAUSTION_COST;
            if saturation > 0.0 {
                saturation = (saturation - 1.0).max(0.0);
            } else if difficulty != Difficulty::Peaceful {
                level = level.saturating_sub(1);
            }
            needs_sync = true;
        }

        if natural_regen && saturation > 0.0 && can_heal && level >= 20 {
            timer += 1;
            if timer >= 10 {
                // `FoodData.tick`: the saturation spent is charged as exhaustion, which the
                // branch above turns into saturation loss at four exhaustion per point.
                // Subtracting it here as well spent it about five times too fast.
                let cost = saturation.min(6.0);
                exhaustion += cost;
                heal_amount = cost / 6.0;
                timer = 0;
                needs_sync = true;
            }
        } else if natural_regen && level >= 18 && can_heal {
            timer += 1;
            if timer >= 80 {
                heal_amount = 1.0;
                exhaustion += 6.0;
                timer = 0;
                needs_sync = true;
            }
        } else if level == 0 {
            timer += 1;
            if timer >= 80 {
                timer = 0;
                let should_starve = match difficulty {
                    Difficulty::Peaceful | Difficulty::Easy => health > 10.0,
                    Difficulty::Normal => health > 1.0,
                    Difficulty::Hard => true,
                };

                if should_starve {
                    damage_amount = 1.0;
                }
                self.tick_timer.store(0);
            }
        } else {
            timer = 0;
        }

        if needs_sync || timer != self.tick_timer.load() {
            self.level.store(level);
            self.saturation.store(saturation);
            self.exhaustion.store(exhaustion);
            self.tick_timer.store(timer);
        }

        if needs_sync {
            player.send_health().await;
        }
        if heal_amount > 0.0 {
            player.heal(heal_amount).await;
        }
        if damage_amount > 0.0 {
            player
                .damage(&**player, damage_amount, DamageType::STARVE)
                .await;
        }
    }

    /// Vanilla `FoodData.add` (`FoodData.java:19-22`): the food sum is a Java `int` add
    /// clamped to `[0, 20]`, then saturation is clamped to `[0, new food level]` with
    /// `Mth.clamp(float)` (`value < min ? min : Math.min(value, max)`).
    fn add(&self, food: i32, saturation: f32) {
        let new_level = food
            .wrapping_add(i32::from(self.level.load()))
            .clamp(0, i32::from(MAX_FOOD));
        let max = new_level as f32;
        let sum = saturation + self.saturation.load();
        let new_sat = if sum < 0.0 {
            0.0
        } else if sum > max {
            max
        } else {
            sum
        };

        // `new_level` lies in `0..=20`, so the narrowing is lossless.
        self.level.store(new_level as u8);
        self.saturation.store(new_sat);
    }

    /// Vanilla `FoodData.eat(int, float)` (`FoodData.java:24-26`), used by cake slices and
    /// the saturation effect: the saturation gained is
    /// `FoodConstants.saturationByModifier(food, modifier)`.
    pub fn eat_with_modifier(&self, food: i32, saturation_modifier: f32) {
        self.add(food, saturation_by_modifier(food, saturation_modifier));
    }

    /// Vanilla `FoodData.eat(FoodProperties)` (`FoodData.java:28-30`), which forwards the
    /// component's `saturation()` straight to `FoodData.add` -- the value is already
    /// absolute, not a modifier, so no `FoodConstants.saturationByModifier` scaling applies.
    pub async fn eat(&self, player: &Player, food: i32, saturation: f32) {
        self.add(food, saturation);
        player.send_health().await;
    }

    /// Add exhaustion to trigger hunger decrease
    pub fn add_exhaustion(&self, exhaustion: f32) {
        let current = self.exhaustion.load();
        self.exhaustion
            .store((current + exhaustion).min(MAX_EXHAUSTION));
    }

    pub fn set_level(&self, level: u8) {
        self.level.store(level.min(MAX_FOOD));
    }

    pub fn set_saturation(&self, saturation: f32) {
        self.saturation
            .store(saturation.min(f32::from(self.level.load())));
    }

    pub fn get_exhaustion(&self) -> f32 {
        self.exhaustion.load()
    }

    pub fn set_exhaustion(&self, exhaustion: f32) {
        self.exhaustion.store(exhaustion.min(MAX_EXHAUSTION));
    }

    pub fn restart(&self) {
        self.level.store(MAX_FOOD);
        self.saturation.store(5.0);
        self.exhaustion.store(0.0);
        self.tick_timer.store(0);
    }

    /// Vanilla `FoodData.hasEnoughFood()` (`FoodData.java:92-94`).
    ///
    /// Returns true if the food level is above 6 (enough to sprint).
    #[must_use]
    pub fn has_enough_food(&self) -> bool {
        self.level.load() > 6
    }

    /// Vanilla `FoodData.needsFood()` (`FoodData.java:96-98`).
    ///
    /// Returns true if the food level is below maximum (20).
    #[must_use]
    pub fn needs_food(&self) -> bool {
        self.level.load() < MAX_FOOD
    }
}

impl NBTStorage for HungerManager {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            nbt.put_int("foodLevel", self.level.load().into());
            nbt.put_float("foodSaturationLevel", self.saturation.load());
            nbt.put_float("foodExhaustionLevel", self.exhaustion.load());
            nbt.put_int("foodTickTimer", self.tick_timer.load() as i32);
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.level
                .store(nbt.get_int("foodLevel").unwrap_or(20) as u8);
            self.saturation
                .store(nbt.get_float("foodSaturationLevel").unwrap_or(5.0));
            self.exhaustion
                .store(nbt.get_float("foodExhaustionLevel").unwrap_or(0.0));
            self.tick_timer
                .store(nbt.get_int("foodTickTimer").unwrap_or(0) as u32);
        })
    }
}

impl NBTStorageInit for HungerManager {}

#[cfg(test)]
mod tests {
    use super::{HungerManager, saturation_by_modifier};

    fn manager(level: u8, saturation: f32) -> HungerManager {
        let manager = HungerManager::default();
        manager.level.store(level);
        manager.saturation.store(saturation);
        manager
    }

    #[test]
    fn cake_modifier_matches_literal() {
        assert_eq!(saturation_by_modifier(2, 0.1).to_bits(), 0.4f32.to_bits());
    }

    #[test]
    fn huge_saturation_effect_clamps() {
        let m = HungerManager::default();
        m.eat_with_modifier(256, 1.0);
        assert_eq!(m.level.load(), 20);
        assert_eq!(m.saturation.load(), 20.0);

        let m = HungerManager::default();
        m.eat_with_modifier(236, 1.0);
        assert_eq!(m.level.load(), 20);
        assert_eq!(m.saturation.load(), 20.0);
    }

    #[test]
    fn cake_slice() {
        let m = manager(10, 3.0);
        m.eat_with_modifier(2, 0.1);
        assert_eq!(m.level.load(), 12);
        assert_eq!(m.saturation.load(), 3.0 + 0.4);

        let m = manager(4, 10.0);
        m.eat_with_modifier(2, 0.1);
        assert_eq!(m.level.load(), 6);
        assert_eq!(m.saturation.load(), 6.0);
    }

    #[test]
    fn add_clamps_like_java() {
        let m = manager(10, 2.0);
        m.add(300, 0.0);
        assert_eq!(m.level.load(), 20);

        let m = manager(10, 2.0);
        m.add(0, -5.0);
        assert_eq!(m.saturation.load(), 0.0);

        // Java `int` addition wraps before the clamp.
        let m = manager(20, 2.0);
        m.add(i32::MAX, 0.0);
        assert_eq!(m.level.load(), 0);
    }
}
