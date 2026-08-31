// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::entity::EntityBase;
use crate::entity::living::LivingEntity;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::item_stack::ItemStack;

/// Utilities for reading potion contents from an `ItemStack` and applying effects.
pub struct PotionContents;

// PotionContents.java:83-103 carries effect type, duration, amplifier, ambient, and visibility
// fields; this tuple is the server-side representation used by the existing effect pipeline.
pub type PotionEffect = (&'static StatusEffect, i32, u8, bool, bool, bool);

// PotionContents.java:235-237 reads POTION_DURATION_SCALE from the stack and defaults to 1.0.
fn potion_duration_scale(stack: &ItemStack) -> f32 {
    stack
        .get_data_component::<pumpkin_data::data_component_impl::PotionDurationScaleImpl>()
        .map_or(1.0, |component| component.scale)
}

/// Source context for applying potion effects (affects scaling rules).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PotionApplicationSource {
    /// Normal application (drinking / splash)
    Normal,
    /// `AreaEffectCloud` application (shorter durations and weaker instant potency)
    AreaEffectCloud,
    Arrow,
}

impl PotionApplicationSource {
    // AreaEffectCloud.java: `effect.getEffect().value().applyInstantaneousEffect(...,
    // effect.getAmplifier(), 0.5)` -- the potency factor is the literal constant 0.5,
    // independent of the cloud's distance-based duration scale, not `scale * 0.5`.
    const fn instant_scale(self, scale: f32) -> f32 {
        match self {
            Self::AreaEffectCloud => 0.5,
            Self::Arrow => 1.0,
            Self::Normal => scale,
        }
    }

    const fn duration_scale(self, scale: f32) -> f32 {
        match self {
            Self::AreaEffectCloud => scale * 0.25,
            Self::Arrow | Self::Normal => scale,
        }
    }
}

impl PotionContents {
    /// Read effects from an `ItemStack`'s `PotionContents` data component.
    #[must_use]
    pub fn read_potion_effects(stack: &ItemStack) -> Vec<PotionEffect> {
        // Prefer generated potion id if present, otherwise use custom_effects
        if let Some(pc) =
            stack.get_data_component::<pumpkin_data::data_component_impl::PotionContentsImpl>()
        {
            // Custom effects present
            let mut out = Vec::new();
            if let Some(potion_id) = pc.potion_id {
                // Map potion id to generated Potion if possible
                macro_rules! try_push_potion {
                    ($p:expr) => {
                        if $p.id as i32 == potion_id {
                            for e in $p.effects {
                                out.push((
                                    e.effect_type,
                                    e.duration,
                                    e.amplifier,
                                    e.ambient,
                                    e.show_particles,
                                    e.show_icon,
                                ));
                            }
                        }
                    };
                }
                try_push_potion!(pumpkin_data::potion::Potion::AWKWARD);
                try_push_potion!(pumpkin_data::potion::Potion::FIRE_RESISTANCE);
                try_push_potion!(pumpkin_data::potion::Potion::HARMING);
                try_push_potion!(pumpkin_data::potion::Potion::HEALING);
                try_push_potion!(pumpkin_data::potion::Potion::INFESTED);
                try_push_potion!(pumpkin_data::potion::Potion::INVISIBILITY);
                try_push_potion!(pumpkin_data::potion::Potion::LEAPING);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_FIRE_RESISTANCE);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_INVISIBILITY);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_LEAPING);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_NIGHT_VISION);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_POISON);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_REGENERATION);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_SLOW_FALLING);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_SLOWNESS);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_STRENGTH);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_SWIFTNESS);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_TURTLE_MASTER);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_WATER_BREATHING);
                try_push_potion!(pumpkin_data::potion::Potion::LONG_WEAKNESS);
                try_push_potion!(pumpkin_data::potion::Potion::LUCK);
                try_push_potion!(pumpkin_data::potion::Potion::MUNDANE);
                try_push_potion!(pumpkin_data::potion::Potion::NIGHT_VISION);
                try_push_potion!(pumpkin_data::potion::Potion::OOZING);
                try_push_potion!(pumpkin_data::potion::Potion::POISON);
                try_push_potion!(pumpkin_data::potion::Potion::REGENERATION);
                try_push_potion!(pumpkin_data::potion::Potion::SLOW_FALLING);
                try_push_potion!(pumpkin_data::potion::Potion::SLOWNESS);
                try_push_potion!(pumpkin_data::potion::Potion::STRENGTH);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_HARMING);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_HEALING);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_LEAPING);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_POISON);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_REGENERATION);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_SLOWNESS);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_STRENGTH);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_SWIFTNESS);
                try_push_potion!(pumpkin_data::potion::Potion::STRONG_TURTLE_MASTER);
                try_push_potion!(pumpkin_data::potion::Potion::SWIFTNESS);
                try_push_potion!(pumpkin_data::potion::Potion::THICK);
                try_push_potion!(pumpkin_data::potion::Potion::TURTLE_MASTER);
                try_push_potion!(pumpkin_data::potion::Potion::WATER);
                try_push_potion!(pumpkin_data::potion::Potion::WATER_BREATHING);
                try_push_potion!(pumpkin_data::potion::Potion::WEAKNESS);
                try_push_potion!(pumpkin_data::potion::Potion::WEAVING);
                try_push_potion!(pumpkin_data::potion::Potion::WIND_CHARGED);
            }

            // Custom effects appended
            for ce in &pc.custom_effects {
                if let Some(se) = StatusEffect::from_minecraft_name(&ce.effect_id) {
                    out.push((
                        se,
                        ce.duration,
                        ce.amplifier as u8,
                        ce.ambient,
                        ce.show_particles,
                        ce.show_icon,
                    ));
                }
            }

            return out;
        }

        Vec::new()
    }

    // PotionContents.java:126-144 averages visible effect colors with amplifier + 1 weight
    // and returns an opaque ARGB color; hidden-particle effects do not contribute.
    #[must_use]
    pub fn get_color_optional(effects: &[PotionEffect]) -> Option<i32> {
        let mut red = 0i64;
        let mut green = 0i64;
        let mut blue = 0i64;
        let mut total_weight = 0i64;

        for &(effect, _, amplifier, _, show_particles, _) in effects {
            if !show_particles {
                continue;
            }

            let weight = i64::from(amplifier) + 1;
            red += i64::from((effect.color >> 16) & 0xff) * weight;
            green += i64::from((effect.color >> 8) & 0xff) * weight;
            blue += i64::from(effect.color & 0xff) * weight;
            total_weight += weight;
        }

        (total_weight > 0).then(|| {
            (0xff00_0000u32
                | ((red / total_weight) as u32) << 16
                | ((green / total_weight) as u32) << 8
                | (blue / total_weight) as u32) as i32
        })
    }

    // PotionContents.java:113-119 uses the supplied default when no custom or visible effect
    // color exists; callers supply the component's custom color before invoking this helper.
    #[must_use]
    pub fn get_color_or(effects: &[PotionEffect], default_color: i32) -> i32 {
        Self::get_color_optional(effects).unwrap_or(default_color)
    }

    // PotionContents.java:154-164 applies every scaled effect when a consumable finishes.
    pub async fn on_consume(target: &LivingEntity, stack: &ItemStack) {
        // PotionContents.java:235-237 reads POTION_DURATION_SCALE from the consumed stack,
        // defaulting to 1.0 when the component is absent.
        let duration_scale = potion_duration_scale(stack);
        let effects = Self::read_potion_effects(stack);
        Self::apply_effects_to_inner(
            target,
            effects,
            duration_scale,
            PotionApplicationSource::Normal,
            1.0,
            false,
        )
        .await;
    }

    /// Apply instant or duration effects to a target living entity.
    pub async fn apply_effects_to(
        target: &LivingEntity,
        effects: Vec<PotionEffect>,
        scale: f32,
        source: PotionApplicationSource,
    ) {
        Self::apply_effects_to_inner(
            target,
            effects,
            scale,
            source,
            source.instant_scale(scale),
            true,
        )
        .await;
    }

    async fn apply_effects_to_inner(
        target: &LivingEntity,
        effects: Vec<PotionEffect>,
        scale: f32,
        source: PotionApplicationSource,
        instant_scale: f32,
        drop_short_duration: bool,
    ) {
        for (effect_type, duration, amplifier, ambient, show_particles, show_icon) in effects {
            // Instant effects should apply immediately
            let is_instant = effect_type.id
                == pumpkin_data::effect::StatusEffect::INSTANT_HEALTH.id
                || effect_type.id == pumpkin_data::effect::StatusEffect::INSTANT_DAMAGE.id;

            if is_instant {
                // Apply instant effects logic directly as they don't tick
                let inverted = target.is_undead();
                if LivingEntity::instant_effect_is_damage(effect_type, inverted) {
                    let amount = (6 * ((amplifier as i32) + 1)) as f32 * instant_scale;

                    target
                        .damage(
                            target.get_entity(),
                            amount,
                            pumpkin_data::damage::DamageType::MAGIC,
                        )
                        .await;
                } else {
                    let amount = (4 * ((amplifier as i32) + 1)) as f32 * instant_scale;
                    target.heal(amount);
                }
                // Vanilla applies instant effects via a direct heal/damage call only
                // (ThrownSplashPotion#applyInstantaneousEffect / HealOrHarmMobEffect), never
                // through addEffect -- calling `target.add_effect` here as well would apply the
                // heal/damage a second time, since `LivingEntity::add_effect` independently
                // re-implements the instant-effect heal/damage branch.
            } else {
                // Vanilla ThrownSplashPotion#applyEffects: duration is rounded (not truncated),
                // and effects whose scaled duration would end within 20 ticks are dropped
                // entirely rather than clamped to a minimum of 1 tick.
                // Area effect clouds scale through PotionContents#forEachEffect ->
                // MobEffectInstance#withScaledDuration (MobEffectInstance.java:191), which
                // floors and clamps up to 1 tick, and AreaEffectCloud.java:233 then adds the
                // effect unconditionally. The `endsWithin(20)` drop is ThrownSplashPotion-only
                // (ThrownSplashPotion.java:67).
                let is_cloud = source == PotionApplicationSource::AreaEffectCloud;
                // MobEffectInstance.mapDuration (MobEffectInstance.java:195-197), which both
                // withScaledDuration (line 191, the cloud path) and ThrownSplashPotion's inline
                // scale (ThrownSplashPotion.java:63) go through: infinite (-1) and zero durations
                // are never scaled.
                let dur = if duration == -1 || duration == 0 {
                    duration
                } else {
                    let scaled = duration as f32 * source.duration_scale(scale);
                    if is_cloud {
                        (scaled.floor() as i32).max(1)
                    } else {
                        (scaled + 0.5) as i32
                    }
                };
                // ThrownSplashPotion.java:67: `!newEffect.endsWithin(20)`, and endsWithin
                // (MobEffectInstance.java:183-184) treats an infinite duration as never
                // ending within any tick count, so it must not be dropped here.
                if drop_short_duration && !is_cloud && dur != -1 && dur <= 20 {
                    continue;
                }
                let eff = pumpkin_data::potion::Effect {
                    effect_type,
                    duration: dur,
                    amplifier,
                    ambient,
                    show_particles,
                    show_icon,
                    blend: false,
                };
                target.add_effect(eff).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PotionApplicationSource, PotionContents, potion_duration_scale};
    use pumpkin_data::data_component_impl::PotionDurationScaleImpl;
    use pumpkin_data::effect::StatusEffect;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn tipped_arrow_scale_shortens_duration_without_reducing_instant_potency() {
        let tipped_arrow = ItemStack::new(1, &Item::TIPPED_ARROW);
        let scale = tipped_arrow
            .get_data_component::<PotionDurationScaleImpl>()
            .expect("tipped arrows should define a potion duration scale")
            .scale;

        assert_eq!(PotionApplicationSource::Arrow.duration_scale(scale), 0.125);
        assert_eq!(
            (160.0 * PotionApplicationSource::Arrow.duration_scale(scale)) as i32,
            20
        );
        assert_eq!(PotionApplicationSource::Arrow.instant_scale(scale), 1.0);
    }

    // PotionContents.java:132-143 includes only visible effects and weights each color by
    // amplifier + 1; this is testable without a live entity or world.
    #[test]
    fn potion_color_uses_visible_amplifier_weighted_effects() {
        let effects = vec![
            (&StatusEffect::INSTANT_HEALTH, 1, 0, false, true, true),
            (&StatusEffect::INSTANT_DAMAGE, 1, 1, false, true, true),
            (&StatusEffect::POISON, 1, 7, false, false, true),
        ];

        let color = PotionContents::get_color_optional(&effects).expect("visible effects color");
        let red = (((StatusEffect::INSTANT_HEALTH.color >> 16) & 0xff)
            + 2 * ((StatusEffect::INSTANT_DAMAGE.color >> 16) & 0xff))
            / 3;
        let green = (((StatusEffect::INSTANT_HEALTH.color >> 8) & 0xff)
            + 2 * ((StatusEffect::INSTANT_DAMAGE.color >> 8) & 0xff))
            / 3;
        let blue = ((StatusEffect::INSTANT_HEALTH.color & 0xff)
            + 2 * (StatusEffect::INSTANT_DAMAGE.color & 0xff))
            / 3;
        let expected = (0xff00_0000u32
            | ((red & 0xff) as u32) << 16
            | ((green & 0xff) as u32) << 8
            | (blue & 0xff) as u32) as i32;

        assert_eq!(color, expected);
        assert_eq!(PotionContents::get_color_or(&[], -7), -7);
    }

    // PotionContents.java:235-237 uses the stack's duration scale and defaults to 1.0.
    #[test]
    fn consumed_potion_duration_scale_comes_from_the_stack() {
        let tipped_arrow = ItemStack::new(1, &Item::TIPPED_ARROW);
        assert_eq!(potion_duration_scale(&tipped_arrow), 0.125);

        let potion = ItemStack::new(1, &Item::POTION);
        assert_eq!(potion_duration_scale(&potion), 1.0);
    }
}
