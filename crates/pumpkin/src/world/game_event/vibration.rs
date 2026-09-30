use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::advancement::Advancement;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use super::{GameEventContext, vibration_frequency};
use crate::entity::EntityBase;

// Mirrors net.minecraft.world.level.gameevent.vibrations.VibrationInfo, except the
// game event itself is replaced by its precomputed vibration frequency: the generated
// pumpkin_data::GameEvent enum (pumpkin-data/src/generated/game_event.rs) derives
// nothing (no Copy/Clone/PartialEq), and that file may not be edited, so it cannot be
// stored and compared across ticks the way vanilla's Holder<GameEvent> is. Selection
// only ever needs the frequency (see `shouldReplaceVibration`), so storing it directly
// loses nothing for this purpose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VibrationInfo {
    pub frequency: i32,
    pub distance: f32,
    pub pos: Vector3<f64>,
    pub source_entity: Option<Uuid>,
    pub projectile_owner: Option<Uuid>,
}

impl VibrationInfo {
    /// Vanilla `VibrationInfo.getProjectileOwner` (`VibrationInfo.java:50-56`).
    ///
    /// Returns the projectile owner UUID if available.
    #[must_use]
    pub const fn projectile_owner(&self) -> Option<Uuid> {
        self.projectile_owner
    }
}

// Port of VibrationSelector.java (66 lines). Picks, per game tick, the single
// candidate vibration a listener will actually act on: closer distance wins, and on
// a distance tie the higher-frequency event wins (VibrationSelector.shouldReplaceVibration).
//
// Vanilla resolves the chosen candidate on a later tick via VibrationSystem.Ticker (travel
// time). The sculk shrieker and the sculk sensors drive it from their block-entity tick;
// `VibrationData` below is the sensors' `VibrationSystem.Data`.
#[derive(Default)]
pub struct VibrationSelector {
    current: Option<(VibrationInfo, u64)>,
}

impl VibrationSelector {
    #[must_use]
    pub const fn new() -> Self {
        Self { current: None }
    }

    pub fn add_candidate(&mut self, candidate: VibrationInfo, tick_time: u64) {
        if self.should_replace_vibration(&candidate, tick_time) {
            self.current = Some((candidate, tick_time));
        }
    }

    fn should_replace_vibration(&self, candidate: &VibrationInfo, tick_time: u64) -> bool {
        let Some((previous, previous_tick)) = &self.current else {
            return true;
        };
        if tick_time != *previous_tick {
            return false;
        }
        if candidate.distance < previous.distance {
            return true;
        }
        if candidate.distance > previous.distance {
            return false;
        }
        candidate.frequency > previous.frequency
    }

    #[must_use]
    pub fn chosen_candidate(&self, time: u64) -> Option<VibrationInfo> {
        let (info, tick) = self.current.as_ref()?;
        (*tick < time).then_some(*info)
    }

    pub const fn start_over(&mut self) {
        self.current = None;
    }

    /// True while no candidate is queued, so a ticker can skip reading the game time.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.current.is_none()
    }

    /// `VibrationSelector.CODEC` (`VibrationSelector.java:8-14`): an optional `event` and a
    /// `tick` that is `-1` while no candidate is queued.
    fn to_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if let Some(event) = self.current.and_then(|(info, _)| info.to_nbt()) {
            nbt.put_compound("event", event);
        }
        let tick = self.current.map_or(-1, |(_, candidate_tick)| {
            i64::try_from(candidate_tick).unwrap_or(i64::MAX)
        });
        nbt.put_long("tick", tick);
        nbt
    }

    /// `event` is lenient (an unreadable one is dropped) but `tick` is required, exactly as in
    /// `VibrationSelector.CODEC`.
    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let tick = nbt.get_long("tick")?;
        let current = nbt
            .get_compound("event")
            .and_then(VibrationInfo::from_nbt)
            .map(|info| (info, u64::try_from(tick).unwrap_or(0)));
        Some(Self { current })
    }
}

/// Every game event of `#minecraft:vibrations` (`data/minecraft/tags/game_event/vibrations.json`)
/// by registry path. `GameEvent` carries no name, so reading a persisted vibration needs this.
static VIBRATION_EVENTS: [(&str, GameEvent); 56] = [
    ("block_attach", GameEvent::BlockAttach),
    ("block_change", GameEvent::BlockChange),
    ("block_close", GameEvent::BlockClose),
    ("block_destroy", GameEvent::BlockDestroy),
    ("block_detach", GameEvent::BlockDetach),
    ("block_open", GameEvent::BlockOpen),
    ("block_place", GameEvent::BlockPlace),
    ("block_activate", GameEvent::BlockActivate),
    ("block_deactivate", GameEvent::BlockDeactivate),
    ("bounce", GameEvent::Bounce),
    ("container_close", GameEvent::ContainerClose),
    ("container_open", GameEvent::ContainerOpen),
    ("drink", GameEvent::Drink),
    ("eat", GameEvent::Eat),
    ("elytra_glide", GameEvent::ElytraGlide),
    ("entity_damage", GameEvent::EntityDamage),
    ("entity_die", GameEvent::EntityDie),
    ("entity_dismount", GameEvent::EntityDismount),
    ("entity_interact", GameEvent::EntityInteract),
    ("entity_mount", GameEvent::EntityMount),
    ("entity_place", GameEvent::EntityPlace),
    ("entity_action", GameEvent::EntityAction),
    ("equip", GameEvent::Equip),
    ("explode", GameEvent::Explode),
    ("fluid_pickup", GameEvent::FluidPickup),
    ("fluid_place", GameEvent::FluidPlace),
    ("hit_ground", GameEvent::HitGround),
    ("instrument_play", GameEvent::InstrumentPlay),
    ("item_interact_finish", GameEvent::ItemInteractFinish),
    ("lightning_strike", GameEvent::LightningStrike),
    ("note_block_play", GameEvent::NoteBlockPlay),
    ("prime_fuse", GameEvent::PrimeFuse),
    ("projectile_land", GameEvent::ProjectileLand),
    ("projectile_shoot", GameEvent::ProjectileShoot),
    ("shear", GameEvent::Shear),
    ("splash", GameEvent::Splash),
    ("step", GameEvent::Step),
    ("swim", GameEvent::Swim),
    ("teleport", GameEvent::Teleport),
    ("unequip", GameEvent::Unequip),
    ("resonate_1", GameEvent::Resonate1),
    ("resonate_2", GameEvent::Resonate2),
    ("resonate_3", GameEvent::Resonate3),
    ("resonate_4", GameEvent::Resonate4),
    ("resonate_5", GameEvent::Resonate5),
    ("resonate_6", GameEvent::Resonate6),
    ("resonate_7", GameEvent::Resonate7),
    ("resonate_8", GameEvent::Resonate8),
    ("resonate_9", GameEvent::Resonate9),
    ("resonate_10", GameEvent::Resonate10),
    ("resonate_11", GameEvent::Resonate11),
    ("resonate_12", GameEvent::Resonate12),
    ("resonate_13", GameEvent::Resonate13),
    ("resonate_14", GameEvent::Resonate14),
    ("resonate_15", GameEvent::Resonate15),
    ("flap", GameEvent::Flap),
];

/// The vibration frequency of a persisted `game_event` key (`minecraft:` is the default
/// namespace), or `None` for an event outside `#minecraft:vibrations`.
fn frequency_for_event_key(key: &str) -> Option<i32> {
    let path = key.strip_prefix("minecraft:").unwrap_or(key);
    VIBRATION_EVENTS
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, event)| vibration_frequency(event))
}

impl VibrationInfo {
    /// `VibrationInfo.CODEC` (`VibrationInfo.java:85-96`).
    ///
    /// Only the frequency of the event is kept (see the note above), and a vibration user
    /// reads nothing else of it (`SculkSensorBlockEntity.java:119-127`), so the event is
    /// written as `resonate_<frequency>`, the `#minecraft:vibrations` event with that
    /// frequency. `None` if the frequency is not one of `1..=15`.
    fn to_nbt(self) -> Option<NbtCompound> {
        if !(1..=15).contains(&self.frequency) {
            return None;
        }
        let mut nbt = NbtCompound::new();
        nbt.put_string(
            "game_event",
            format!("minecraft:resonate_{}", self.frequency),
        );
        nbt.put_float("distance", self.distance);
        nbt.put_list(
            "pos",
            vec![
                NbtTag::Double(self.pos.x),
                NbtTag::Double(self.pos.y),
                NbtTag::Double(self.pos.z),
            ],
        );
        if let Some(source) = self.source_entity {
            nbt.put_uuid("source", source);
        }
        if let Some(owner) = self.projectile_owner {
            nbt.put_uuid("projectile_owner", owner);
        }
        Some(nbt)
    }

    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let frequency = frequency_for_event_key(nbt.get_string("game_event")?)?;
        let distance = nbt
            .get_float("distance")
            .filter(|distance| *distance >= 0.0)?;
        let [x, y, z] = nbt.get_list("pos")? else {
            return None;
        };
        Some(Self {
            frequency,
            distance,
            pos: Vector3::new(
                x.extract_double()?,
                y.extract_double()?,
                z.extract_double()?,
            ),
            source_entity: nbt.get_uuid("source"),
            projectile_owner: nbt.get_uuid("projectile_owner"),
        })
    }
}

/// `VibrationSystem.Data` (`VibrationSystem.java:123-190`) without `reloadVibrationParticle`:
/// the travelling-vibration particle is not sent to clients.
#[derive(Default)]
pub struct VibrationData {
    pub selector: VibrationSelector,
    pub current_vibration: Option<VibrationInfo>,
    pub travel_time: u32,
}

impl VibrationData {
    /// `VibrationSystem.Ticker.trySelectAndScheduleVibration` (`VibrationSystem.java:300-315`):
    /// promotes the selector's candidate, if one is due, to the vibration in flight. Returns
    /// whether it did (vanilla calls `onDataChanged` then).
    pub fn select_and_schedule(&mut self, game_time: u64) -> bool {
        let Some(vibration) = self.selector.chosen_candidate(game_time) else {
            return false;
        };
        self.travel_time = travel_time_in_ticks(vibration.distance);
        self.current_vibration = Some(vibration);
        self.selector.start_over();
        true
    }

    /// `VibrationSystem.Data.CODEC` (`VibrationSystem.java:124-136`), stored under the
    /// `listener` key (`VibrationSystem.Data.NBT_TAG_KEY`).
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if let Some(event) = self.current_vibration.and_then(VibrationInfo::to_nbt) {
            nbt.put_compound("event", event);
        }
        nbt.put_compound("selector", self.selector.to_nbt());
        nbt.put_int(
            "event_delay",
            i32::try_from(self.travel_time).unwrap_or(i32::MAX),
        );
        nbt
    }

    /// `None` where vanilla's `read` fails and the block entity falls back to an empty `Data`
    /// (`SculkSensorBlockEntity.java:44`): no `selector`, or a negative `event_delay`.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let selector = VibrationSelector::from_nbt(nbt.get_compound("selector")?)?;
        let travel_time = nbt
            .get_int("event_delay")
            .map_or(Some(0), |delay| u32::try_from(delay).ok())?;
        Some(Self {
            selector,
            current_vibration: nbt.get_compound("event").and_then(VibrationInfo::from_nbt),
            travel_time,
        })
    }
}

/// `VibrationSystem.User.calculateTravelTimeInTicks` (`VibrationSystem.java:401-403`).
#[must_use]
pub const fn travel_time_in_ticks(distance: f32) -> u32 {
    distance.floor().max(0.0) as u32
}

/// `#minecraft:ignore_vibrations_sneaking` (`data/minecraft/tags/game_event/
/// ignore_vibrations_sneaking.json`), matched directly because `GameEvent` carries no
/// `Taggable` impl.
const fn ignored_when_sneaking(event: &GameEvent) -> bool {
    matches!(
        event,
        GameEvent::HitGround
            | GameEvent::ProjectileShoot
            | GameEvent::Step
            | GameEvent::Swim
            | GameEvent::ItemInteractStart
            | GameEvent::ItemInteractFinish
    )
}

/// `Entity.isSteppingCarefully` (`Entity.java:2681`) is the shift-key flag; `Cat` and `Ocelot`
/// also count `isCrouching()`, the `CROUCHING` pose of a stalking feline
/// (`Cat.java:496-498`, `Ocelot.java:277-279`).
fn is_stepping_carefully(source: &dyn EntityBase) -> bool {
    let entity = source.get_entity();
    entity.is_sneaking()
        || ((entity.entity_type == &EntityType::CAT || entity.entity_type == &EntityType::OCELOT)
            && matches!(entity.pose.load(), EntityPose::Crouching))
}

/// `Entity.dampensVibrations` (`Entity.java:1537`): only wardens (`Warden.java:195`) and
/// item entities holding a `#minecraft:dampens_vibrations` item (`ItemEntity.java:85-87`).
async fn dampens_vibrations(source: &Arc<dyn EntityBase>) -> bool {
    if source.get_entity().entity_type == &EntityType::WARDEN {
        return true;
    }
    if let Some(item) = source.clone().get_item_entity() {
        return item.dampens_vibrations().await;
    }
    false
}

/// `VibrationSystem.User.isValidVibration` (`VibrationSystem.java:405-430`).
///
/// For a user that listens to `#minecraft:vibrations`, the default `getListenableEvents`: the
/// tag holds exactly the events with a vibration frequency (`vibration_frequency`).
///
/// A sneaking player whose event is ignored earns the `avoid_vibration` advancement when the
/// user `canTriggerAvoidVibration`.
pub async fn is_valid_vibration(
    event: &GameEvent,
    context: &GameEventContext,
    can_trigger_avoid_vibration: bool,
) -> bool {
    if vibration_frequency(event) == 0 {
        return false;
    }
    if let Some(source) = &context.source_entity {
        if source.is_spectator() {
            return false;
        }
        if is_stepping_carefully(source.as_ref()) && ignored_when_sneaking(event) {
            if can_trigger_avoid_vibration && let Some(player) = source.get_player() {
                player
                    .trigger_advancement_criterion(
                        Advancement::ADVENTURE_AVOID_VIBRATION,
                        "avoid_vibration",
                    )
                    .await;
            }
            return false;
        }
        if dampens_vibrations(source).await {
            return false;
        }
    }
    context.affected_block_state.is_none_or(|state| {
        !Block::from_state_id(state).has_tag(&tag::Block::MINECRAFT_DAMPENS_VIBRATIONS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(frequency: i32, distance: f32) -> VibrationInfo {
        VibrationInfo {
            frequency,
            distance,
            pos: Vector3::new(0.0, 0.0, 0.0),
            source_entity: None,
            projectile_owner: None,
        }
    }

    #[test]
    fn vibration_event_table_is_exactly_the_vibrations_tag() {
        // `#minecraft:vibrations` is the set of events with a vibration frequency.
        let tag_names = tag::GameEvent::MINECRAFT_VIBRATIONS.0;
        assert_eq!(VIBRATION_EVENTS.len(), tag_names.len());
        for (name, event) in &VIBRATION_EVENTS {
            assert!(tag_names.contains(name), "{name} is not in the tag");
            assert_ne!(vibration_frequency(event), 0, "{name} has no frequency");
        }
        assert_eq!(frequency_for_event_key("minecraft:step"), Some(1));
        assert_eq!(frequency_for_event_key("resonate_15"), Some(15));
        assert_eq!(frequency_for_event_key("minecraft:explode"), Some(15));
        assert_eq!(frequency_for_event_key("minecraft:shriek"), None);
    }

    #[test]
    fn sneaking_ignores_exactly_the_tagged_events() {
        let tag_names = tag::GameEvent::MINECRAFT_IGNORE_VIBRATIONS_SNEAKING.0;
        for (name, event) in &VIBRATION_EVENTS {
            assert_eq!(
                ignored_when_sneaking(event),
                tag_names.contains(name),
                "{name}"
            );
        }
        // `item_interact_start` is tagged but carries no vibration frequency.
        assert!(ignored_when_sneaking(&GameEvent::ItemInteractStart));
        assert!(!ignored_when_sneaking(&GameEvent::BlockDestroy));
    }

    #[test]
    fn travel_time_floors_the_distance() {
        // `Mth.floor` (`VibrationSystem.java:401-403`).
        assert_eq!(travel_time_in_ticks(0.9), 0);
        assert_eq!(travel_time_in_ticks(7.99), 7);
        assert_eq!(travel_time_in_ticks(8.0), 8);
    }

    #[test]
    fn select_and_schedule_promotes_only_an_earlier_tick_candidate() {
        let mut data = VibrationData::default();
        data.selector.add_candidate(info(5, 3.7), 10);
        assert!(!data.select_and_schedule(10));
        assert!(data.current_vibration.is_none());

        assert!(data.select_and_schedule(11));
        assert_eq!(data.current_vibration, Some(info(5, 3.7)));
        assert_eq!(data.travel_time, 3);
        assert!(data.selector.is_empty());
    }

    #[test]
    fn vibration_data_survives_an_nbt_round_trip() {
        let source = Uuid::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        let mut data = VibrationData {
            current_vibration: Some(VibrationInfo {
                frequency: 9,
                distance: 4.25,
                pos: Vector3::new(1.5, -64.0, 30.75),
                source_entity: Some(source),
                projectile_owner: None,
            }),
            travel_time: 3,
            ..VibrationData::default()
        };
        data.selector.add_candidate(info(2, 6.5), 4_000_000_000);

        let restored = VibrationData::from_nbt(&data.to_nbt()).expect("round trip");
        assert_eq!(restored.current_vibration, data.current_vibration);
        assert_eq!(restored.travel_time, 3);
        assert_eq!(
            restored.selector.chosen_candidate(4_000_000_001),
            Some(info(2, 6.5))
        );
        assert_eq!(restored.selector.chosen_candidate(4_000_000_000), None);
    }

    #[test]
    fn empty_vibration_data_round_trips_and_missing_selector_is_rejected() {
        let restored = VibrationData::from_nbt(&VibrationData::default().to_nbt())
            .expect("empty data round trips");
        assert!(restored.current_vibration.is_none());
        assert!(restored.selector.is_empty());
        assert_eq!(restored.travel_time, 0);

        // `selector` is a required field of `VibrationSystem.Data.CODEC`.
        assert!(VibrationData::from_nbt(&NbtCompound::new()).is_none());
        // A negative `event_delay` fails `NON_NEGATIVE_INT`.
        let mut nbt = VibrationData::default().to_nbt();
        nbt.put_int("event_delay", -1);
        assert!(VibrationData::from_nbt(&nbt).is_none());
    }

    #[test]
    fn unreadable_event_is_dropped_leniently() {
        let data = VibrationData {
            current_vibration: Some(info(4, 1.0)),
            ..VibrationData::default()
        };
        let mut nbt = data.to_nbt();
        let mut event = NbtCompound::new();
        event.put_string("game_event", "minecraft:shriek".to_string());
        nbt.put_compound("event", event);

        let restored = VibrationData::from_nbt(&nbt).expect("event is lenient");
        assert!(restored.current_vibration.is_none());
    }

    #[test]
    fn first_candidate_is_always_accepted() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(1, 5.0), 10);
        assert_eq!(selector.chosen_candidate(11), Some(info(1, 5.0)));
    }

    #[test]
    fn not_chosen_before_the_tick_after_selection() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(1, 5.0), 10);
        assert_eq!(selector.chosen_candidate(10), None);
        assert_eq!(selector.chosen_candidate(9), None);
    }

    #[test]
    fn closer_candidate_replaces_further_one_on_the_same_tick() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(1, 10.0), 10);
        selector.add_candidate(info(1, 2.0), 10);
        assert_eq!(selector.chosen_candidate(11), Some(info(1, 2.0)));
    }

    #[test]
    fn further_candidate_does_not_replace_closer_one_on_the_same_tick() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(1, 2.0), 10);
        selector.add_candidate(info(1, 10.0), 10);
        assert_eq!(selector.chosen_candidate(11), Some(info(1, 2.0)));
    }

    #[test]
    fn equal_distance_prefers_higher_frequency() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(3, 5.0), 10);
        selector.add_candidate(info(9, 5.0), 10);
        assert_eq!(selector.chosen_candidate(11), Some(info(9, 5.0)));

        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(9, 5.0), 10);
        selector.add_candidate(info(3, 5.0), 10);
        assert_eq!(selector.chosen_candidate(11), Some(info(9, 5.0)));
    }

    #[test]
    fn candidate_on_a_different_tick_does_not_replace_the_current_one() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(1, 5.0), 10);
        selector.add_candidate(info(15, 0.1), 11);
        assert_eq!(selector.chosen_candidate(11), Some(info(1, 5.0)));
    }

    #[test]
    fn start_over_clears_the_selection() {
        let mut selector = VibrationSelector::new();
        selector.add_candidate(info(1, 5.0), 10);
        selector.start_over();
        assert_eq!(selector.chosen_candidate(11), None);
    }
}
