use pumpkin_protocol::{bedrock::client::set_time::CSetTime, java::client::play::CUpdateTime};

use super::World;

#[derive(Clone, Debug, PartialEq)]
pub struct ClockInstance {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
}

impl Default for ClockInstance {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockInstance {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_ticks: 0,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
        }
    }

    pub const fn load_from(
        &mut self,
        total_ticks: i64,
        partial_tick: f32,
        rate: f32,
        paused: bool,
    ) {
        self.total_ticks = total_ticks;
        self.partial_tick = partial_tick;
        self.rate = rate;
        self.paused = paused;
    }

    pub fn tick(&mut self) {
        if !self.paused {
            self.partial_tick += self.rate;
            let full_ticks = self.partial_tick.floor() as i32;
            self.partial_tick -= full_ticks as f32;
            self.total_ticks += full_ticks as i64;
        }
    }

    pub const fn set_total_ticks(&mut self, total_ticks: i64) {
        self.total_ticks = total_ticks;
        self.partial_tick = 0.0;
    }

    pub fn add_ticks(&mut self, ticks: i64) {
        self.total_ticks = (self.total_ticks + ticks).max(0);
    }

    pub const fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub const fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
    }

    #[must_use]
    pub const fn pack_network_state(&self, advance_time: bool) -> (i64, f32, f32) {
        let paused = self.paused || !advance_time;
        let rate = if paused { 0.0 } else { self.rate };
        (self.total_ticks, self.partial_tick, rate)
    }
}

#[derive(Clone, Debug)]
pub struct LevelTime {
    pub time_of_day: i64,
    pub world_age: i64,
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
}

impl Default for LevelTime {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelTime {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            time_of_day: 0,
            world_age: 0,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
        }
    }

    #[must_use]
    pub const fn with_time_of_day(time_of_day: i64) -> Self {
        Self {
            world_age: 0,
            time_of_day,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
        }
    }

    pub const fn tick_time(&mut self, advance_time: bool, _advance_weather: bool) {
        self.world_age += 1;
        if advance_time && !self.paused {
            self.partial_tick += self.rate;
            let full_ticks = self.partial_tick.floor() as i32;
            self.partial_tick -= full_ticks as f32;
            self.time_of_day += full_ticks as i64;
        }
    }

    pub const fn load_from(&mut self, time_of_day: i64, world_age: i64) {
        self.time_of_day = time_of_day;
        self.world_age = world_age;
    }

    pub fn tick(&mut self, advance_time: bool) {
        self.tick_time(advance_time, false);
    }

    pub async fn send_time(&self, world: &World) {
        let advance_time = {
            let lock = world.level_info.load();
            lock.game_rules.advance_time
        };

        let (total_ticks, partial_tick, rate) = self.pack_network_state(advance_time);

        world
            .broadcast_editioned(
                &CUpdateTime::new_clock(self.world_age, 0, total_ticks, partial_tick, rate),
                &CSetTime::new(self.time_of_day as _), // TODO do we need to tell bedrock that time is frozen?
            )
            .await;
    }

    pub fn add_time(&mut self, time: i64) {
        self.time_of_day = (self.time_of_day + time).max(0);
    }

    pub const fn set_time(&mut self, time: i64) {
        self.time_of_day = time;
        self.partial_tick = 0.0;
    }

    pub const fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub const fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
    }

    #[must_use]
    pub const fn pack_network_state(&self, advance_time: bool) -> (i64, f32, f32) {
        let paused = self.paused || !advance_time;
        let rate = if paused { 0.0 } else { self.rate };
        (self.time_of_day, self.partial_tick, rate)
    }

    #[must_use]
    pub const fn query_daytime(&self) -> i64 {
        self.time_of_day % 24000
    }

    #[must_use]
    pub const fn query_gametime(&self) -> i64 {
        self.world_age
    }

    #[must_use]
    pub const fn query_day(&self) -> i64 {
        self.time_of_day / 24000
    }

    #[must_use]
    pub const fn is_night(&self) -> bool {
        (self.time_of_day % 24000) >= 12000 && (self.time_of_day % 24000) <= 23999
    }

    /// Whether this tick opens the persistent (`CREATURE`) mob-category spawn
    /// gate.
    ///
    /// Mirrors vanilla's `ServerChunkCache.tickChunks`:
    /// `boolean spawnPersistent = this.level.getGameTime() % 400L == 0L;`.
    /// `getGameTime()` is backed by the unconditionally-incrementing game
    /// time (`world_age` here), not the day/night clock (`time_of_day`),
    /// which the `doDaylightCycle` game rule can freeze and which sleep-skips
    /// / `/time set` can jump independently of elapsed ticks.
    #[must_use]
    pub const fn opens_persistent_spawn_gate(&self) -> bool {
        self.world_age % 400 == 0
    }
}

#[cfg(test)]
mod test {
    use super::LevelTime;

    #[test]
    fn restores_persisted_time_of_day() {
        let time = LevelTime::with_time_of_day(36_001);

        assert_eq!(time.time_of_day, 36_001);
        assert_eq!(time.world_age, 0);
    }

    /// `opens_persistent_spawn_gate` (the function `World::tick` calls to
    /// decide `spawn_passives`) must key off `world_age`, which keeps
    /// advancing every tick regardless of the `doDaylightCycle` game rule
    /// (`advance_time`) — unlike `time_of_day`, which freezes when that rule
    /// is off and can also jump independently via sleep-skips / `/time set`.
    ///
    /// If the gate were keyed off `time_of_day` instead, disabling the
    /// daylight cycle would permanently stop `CREATURE`-category natural
    /// spawns (cows, sheep, pigs, chickens, horses, ...) the moment
    /// `time_of_day` settled on a non-multiple of 400, since it would then
    /// never change again.
    #[test]
    fn persistent_spawn_gate_opens_on_world_age_even_when_time_of_day_is_frozen() {
        let mut time = LevelTime::new();

        for _ in 0..500 {
            time.tick_time(false, false); // advance_time = false: doDaylightCycle off
        }

        assert_eq!(time.time_of_day, 0);
        assert_eq!(time.world_age, 500);
        assert!(!time.opens_persistent_spawn_gate());

        for _ in 0..300 {
            time.tick_time(false, false);
        }

        assert_eq!(time.time_of_day, 0, "frozen regardless of world_age");
        assert_eq!(time.world_age, 800);
        assert!(time.opens_persistent_spawn_gate());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_instance_ticking() {
        let mut clock = ClockInstance::new();
        assert_eq!(clock.total_ticks, 0);
        assert_eq!(clock.partial_tick, 0.0);
        assert_eq!(clock.rate, 1.0);
        assert!(!clock.paused);

        // Standard tick at rate 1.0
        clock.tick();
        assert_eq!(clock.total_ticks, 1);
        assert_eq!(clock.partial_tick, 0.0);

        // Half rate tick
        clock.set_rate(0.5);
        clock.tick();
        assert_eq!(clock.total_ticks, 1);
        assert_eq!(clock.partial_tick, 0.5);
        clock.tick();
        assert_eq!(clock.total_ticks, 2);
        assert_eq!(clock.partial_tick, 0.0);

        // Double rate tick
        clock.set_rate(2.0);
        clock.tick();
        assert_eq!(clock.total_ticks, 4);
        assert_eq!(clock.partial_tick, 0.0);

        // Paused clock
        clock.set_paused(true);
        clock.tick();
        assert_eq!(clock.total_ticks, 4);
    }

    #[test]
    fn level_time_set_and_add() {
        let mut time = LevelTime::new();
        time.set_time(1000);
        assert_eq!(time.time_of_day, 1000);
        assert_eq!(time.partial_tick, 0.0);

        time.add_time(500);
        assert_eq!(time.time_of_day, 1500);

        time.add_time(-2000);
        assert_eq!(time.time_of_day, 0);
    }
}
