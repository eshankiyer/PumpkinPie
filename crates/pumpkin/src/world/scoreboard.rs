use std::collections::{HashMap, HashSet};

use pumpkin_data::scoreboard::ScoreboardDisplaySlot;
use pumpkin_protocol::{
    BClientPacket, ClientPacket, NumberFormat,
    bedrock::client::scoreboard::{
        CRemoveObjective as BRemoveObjective, CSetDisplayObjective as BSetDisplayObjective,
        CSetScore as BSetScore, ScoreEntry as BScoreEntry,
    },
    codec::var_int::VarInt,
    java::client::play::{
        CDisplayObjective, CResetScore, CSetPlayerTeam, CUpdateObjectives, CUpdateScore, Mode,
        RenderType, TeamMethod, TeamParameters,
    },
};
use pumpkin_util::text::{TextComponent, color::NamedColor};
use tracing::warn;

use super::World;
use crate::entity::player::Player;
use crate::net::ClientPlatform;

/// Returns `true` if the criterion name is valid (either a known built-in
/// or matches the `stat.*` or `teamkill.*` / `killedByTeam.*` patterns).
#[must_use]
pub fn is_valid_criterion(criterion: &str) -> bool {
    const BUILT_IN_CRITERIA: &[&str] = &[
        "dummy",
        "trigger",
        "deathCount",
        "playerKillCount",
        "totalKillCount",
        "health",
        "food",
        "air",
        "armor",
        "xp",
        "level",
    ];
    const TEAM_COLORS: &[&str] = &[
        "black",
        "dark_blue",
        "dark_green",
        "dark_aqua",
        "dark_red",
        "dark_purple",
        "gold",
        "gray",
        "dark_gray",
        "blue",
        "green",
        "aqua",
        "red",
        "light_purple",
        "yellow",
        "white",
    ];

    if BUILT_IN_CRITERIA.contains(&criterion) {
        return true;
    }

    // Team kill criteria: teamkill.<color> or killedByTeam.<color>
    for color in TEAM_COLORS {
        let teamkill_crit = format!("teamkill.{color}");
        let killed_crit = format!("killedByTeam.{color}");
        if criterion == teamkill_crit || criterion == killed_crit {
            return true;
        }
    }

    // Stat criteria: stat.<stat_type>.<stat>
    let parts: Vec<&str> = criterion.split('.').collect();
    if parts.len() >= 3 && parts[0] == "stat" {
        return true;
    }

    false
}

/// Returns the default [`RenderType`] for a built-in criterion.
///
/// Only `"health"` defaults to `Hearts`; all others default to `Integer`.
#[must_use]
pub fn default_render_type_for_criterion(criterion: &str) -> RenderType {
    match criterion {
        "health" => RenderType::Hearts,
        _ => RenderType::Integer,
    }
}

/// Returns the string name for a display slot.
/// Used as the NBT key in `scoreboard.dat` (NBT only supports string keys).
#[must_use]
pub const fn display_slot_name(slot: ScoreboardDisplaySlot) -> &'static str {
    match slot {
        ScoreboardDisplaySlot::List => "list",
        ScoreboardDisplaySlot::Sidebar => "sidebar",
        ScoreboardDisplaySlot::BelowName => "below_name",
        ScoreboardDisplaySlot::TeamBlack => "sidebar.team.black",
        ScoreboardDisplaySlot::TeamDarkBlue => "sidebar.team.dark_blue",
        ScoreboardDisplaySlot::TeamDarkGreen => "sidebar.team.dark_green",
        ScoreboardDisplaySlot::TeamDarkAqua => "sidebar.team.dark_aqua",
        ScoreboardDisplaySlot::TeamDarkRed => "sidebar.team.dark_red",
        ScoreboardDisplaySlot::TeamDarkPurple => "sidebar.team.dark_purple",
        ScoreboardDisplaySlot::TeamGold => "sidebar.team.gold",
        ScoreboardDisplaySlot::TeamGray => "sidebar.team.gray",
        ScoreboardDisplaySlot::TeamDarkGray => "sidebar.team.dark_gray",
        ScoreboardDisplaySlot::TeamBlue => "sidebar.team.blue",
        ScoreboardDisplaySlot::TeamGreen => "sidebar.team.green",
        ScoreboardDisplaySlot::TeamAqua => "sidebar.team.aqua",
        ScoreboardDisplaySlot::TeamRed => "sidebar.team.red",
        ScoreboardDisplaySlot::TeamLightPurple => "sidebar.team.light_purple",
        ScoreboardDisplaySlot::TeamYellow => "sidebar.team.yellow",
        ScoreboardDisplaySlot::TeamWhite => "sidebar.team.white",
    }
}

/// Parses a display slot from its string name (inverse of [`display_slot_name`]).
#[must_use]
pub fn display_slot_from_name(name: &str) -> Option<ScoreboardDisplaySlot> {
    match name {
        "list" => Some(ScoreboardDisplaySlot::List),
        "sidebar" => Some(ScoreboardDisplaySlot::Sidebar),
        "below_name" => Some(ScoreboardDisplaySlot::BelowName),
        "sidebar.team.black" => Some(ScoreboardDisplaySlot::TeamBlack),
        "sidebar.team.dark_blue" => Some(ScoreboardDisplaySlot::TeamDarkBlue),
        "sidebar.team.dark_green" => Some(ScoreboardDisplaySlot::TeamDarkGreen),
        "sidebar.team.dark_aqua" => Some(ScoreboardDisplaySlot::TeamDarkAqua),
        "sidebar.team.dark_red" => Some(ScoreboardDisplaySlot::TeamDarkRed),
        "sidebar.team.dark_purple" => Some(ScoreboardDisplaySlot::TeamDarkPurple),
        "sidebar.team.gold" => Some(ScoreboardDisplaySlot::TeamGold),
        "sidebar.team.gray" => Some(ScoreboardDisplaySlot::TeamGray),
        "sidebar.team.dark_gray" => Some(ScoreboardDisplaySlot::TeamDarkGray),
        "sidebar.team.blue" => Some(ScoreboardDisplaySlot::TeamBlue),
        "sidebar.team.green" => Some(ScoreboardDisplaySlot::TeamGreen),
        "sidebar.team.aqua" => Some(ScoreboardDisplaySlot::TeamAqua),
        "sidebar.team.red" => Some(ScoreboardDisplaySlot::TeamRed),
        "sidebar.team.light_purple" => Some(ScoreboardDisplaySlot::TeamLightPurple),
        "sidebar.team.yellow" => Some(ScoreboardDisplaySlot::TeamYellow),
        "sidebar.team.white" => Some(ScoreboardDisplaySlot::TeamWhite),
        _ => None,
    }
}

/// Vanilla `TeamColor#getSerializedName()` string for a [`NamedColor`], used for the
/// `TeamColor` field of a persisted team (`net.minecraft.world.scores.TeamColor`).
#[must_use]
pub const fn named_color_to_str(color: NamedColor) -> &'static str {
    match color {
        NamedColor::Black => "black",
        NamedColor::DarkBlue => "dark_blue",
        NamedColor::DarkGreen => "dark_green",
        NamedColor::DarkAqua => "dark_aqua",
        NamedColor::DarkRed => "dark_red",
        NamedColor::DarkPurple => "dark_purple",
        NamedColor::Gold => "gold",
        NamedColor::Gray => "gray",
        NamedColor::DarkGray => "dark_gray",
        NamedColor::Blue => "blue",
        NamedColor::Green => "green",
        NamedColor::Aqua => "aqua",
        NamedColor::Red => "red",
        NamedColor::LightPurple => "light_purple",
        NamedColor::Yellow => "yellow",
        NamedColor::White => "white",
    }
}

#[allow(async_fn_in_trait)]
pub trait ScoreboardTarget: Send + Sync {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    );
    async fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J);
}

impl ScoreboardTarget for World {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        self.broadcast_editioned(je_packet, be_packet).await;
    }

    async fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        self.broadcast_packet_all(je_packet);
    }
}

impl<T: ScoreboardTarget + ?Sized> ScoreboardTarget for &T {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        (*self).send_editioned(je_packet, be_packet).await;
    }

    async fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        (*self).send_je(je_packet).await;
    }
}

impl ScoreboardTarget for std::sync::Arc<World> {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        self.broadcast_editioned(je_packet, be_packet).await;
    }

    async fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        self.broadcast_packet_all(je_packet);
    }
}

impl ScoreboardTarget for Player {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        Self::send_editioned(self, je_packet, be_packet).await;
    }

    async fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        Self::send_client_packet(self, je_packet).await;
    }
}

impl ScoreboardTarget for std::sync::Arc<Player> {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        je_packet: &J,
        be_packet: &B,
    ) {
        Player::send_editioned(self, je_packet, be_packet).await;
    }

    async fn send_je<J: ClientPacket + Sync>(&self, je_packet: &J) {
        Player::send_client_packet(self, je_packet).await;
    }
}

pub struct NoTarget;

impl ScoreboardTarget for NoTarget {
    async fn send_editioned<J: ClientPacket + Sync, B: BClientPacket + Sync>(
        &self,
        _je_packet: &J,
        _be_packet: &B,
    ) {
    }

    async fn send_je<J: ClientPacket + Sync>(&self, _je_packet: &J) {}
}

#[derive(Clone, Debug, Default)]
pub struct Scoreboard {
    objectives: HashMap<String, ScoreboardObjective>,
    display_slots: HashMap<ScoreboardDisplaySlot, String>,
    scores: HashMap<String, HashMap<String, ScoreboardScore>>,
    teams: HashMap<String, Team>,
    /// Objectives currently known to clients. Ours' criterion/number-format helpers only
    /// broadcast for these.
    tracked_objectives: HashSet<String>,
    /// Reverse index from criterion name -> objective names. Enables efficient updates of all
    /// objectives tracking a given auto-computed criterion (health, food, ...).
    objectives_by_criterion: HashMap<String, Vec<String>>,
}

impl Scoreboard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn get_objectives(&self) -> &HashMap<String, ScoreboardObjective> {
        &self.objectives
    }

    #[must_use]
    pub fn get_objective(&self, name: &str) -> Option<&ScoreboardObjective> {
        self.objectives.get(name)
    }

    pub fn get_objective_mut(&mut self, name: &str) -> Option<&mut ScoreboardObjective> {
        self.objectives.get_mut(name)
    }

    #[must_use]
    pub const fn get_display_slots(&self) -> &HashMap<ScoreboardDisplaySlot, String> {
        &self.display_slots
    }

    #[must_use]
    pub fn get_display_objective(&self, slot: ScoreboardDisplaySlot) -> Option<&str> {
        self.display_slots.get(&slot).map(String::as_str)
    }

    #[must_use]
    pub const fn get_scores(&self) -> &HashMap<String, HashMap<String, ScoreboardScore>> {
        &self.scores
    }

    #[must_use]
    pub fn get_score(&self, entity_name: &str, objective_name: &str) -> Option<&ScoreboardScore> {
        self.scores.get(objective_name)?.get(entity_name)
    }

    #[must_use]
    pub fn get_score_value(&self, entity_name: &str, objective_name: &str) -> Option<i32> {
        self.get_score(entity_name, objective_name)
            .map(|s| s.value.0)
    }

    #[must_use]
    pub fn get_scores_for_objective(
        &self,
        objective_name: &str,
    ) -> Option<&HashMap<String, ScoreboardScore>> {
        self.scores.get(objective_name)
    }

    #[must_use]
    pub fn get_scores_for_entity(&self, entity_name: &str) -> HashMap<String, &ScoreboardScore> {
        let mut entity_scores = HashMap::new();
        for (obj_name, obj_scores) in &self.scores {
            if let Some(score) = obj_scores.get(entity_name) {
                entity_scores.insert(obj_name.clone(), score);
            }
        }
        entity_scores
    }

    #[must_use]
    pub const fn get_teams(&self) -> &HashMap<String, Team> {
        &self.teams
    }

    #[must_use]
    pub fn get_team(&self, name: &str) -> Option<&Team> {
        self.teams.get(name)
    }

    pub fn get_team_mut(&mut self, name: &str) -> Option<&mut Team> {
        self.teams.get_mut(name)
    }

    #[must_use]
    pub fn get_entity_team(&self, entity_name: &str) -> Option<&Team> {
        self.teams
            .values()
            .find(|team| team.players.iter().any(|p| p == entity_name))
    }

    pub fn add_objective(&mut self, objective: ScoreboardObjective) {
        if self.objectives.contains_key(&objective.name) {
            warn!(
                "Tried to create an objective which already exists: {}",
                &objective.name
            );
            return;
        }

        // `ServerScoreboard.addObjective` does NOT send anything: an objective only reaches
        // clients once it is assigned to a display slot, which is where vanilla calls
        // `startTrackingObjective` (`ServerScoreboard.java:105-113`). Sending here would show
        // scoreboards the player should not see yet.
        self.objectives_by_criterion
            .entry(objective.criterion.clone())
            .or_default()
            .push(objective.name.clone());
        self.objectives.insert(objective.name.clone(), objective);
    }

    pub async fn update_objective(
        &mut self,
        target: &impl ScoreboardTarget,
        objective: ScoreboardObjective,
    ) {
        if !self.objectives.contains_key(&objective.name) {
            warn!(
                "Tried to update an objective which does not exist: {}",
                &objective.name
            );
            return;
        }

        let je_update = CUpdateObjectives::new(
            objective.name.clone(),
            Mode::Update,
            objective.display_name.clone(),
            objective.render_type,
            objective.number_format.clone(),
        );

        let be_update = BSetDisplayObjective {
            display_slot: "sidebar".to_string(),
            objective_name: objective.name.clone(),
            display_name: objective.display_name.clone().get_text(),
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(0),
        };

        target.send_editioned(&je_update, &be_update).await;

        self.objectives.insert(objective.name.clone(), objective);
    }

    pub async fn set_display_objective(
        &mut self,
        target: &impl ScoreboardTarget,
        slot: ScoreboardDisplaySlot,
        objective_name: Option<&str>,
    ) {
        let slot_str = match slot {
            ScoreboardDisplaySlot::List => "list",
            ScoreboardDisplaySlot::BelowName => "belowname",
            _ => "sidebar",
        };

        let obj_name_str = objective_name.unwrap_or("");

        let display_name = objective_name
            .and_then(|name| self.objectives.get(name))
            .map_or_else(
                || obj_name_str.to_string(),
                |o| o.display_name.clone().get_text(),
            );

        let je_display = CDisplayObjective::new(slot, obj_name_str.to_string());
        let be_display = BSetDisplayObjective {
            display_slot: slot_str.to_string(),
            objective_name: obj_name_str.to_string(),
            display_name,
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(0),
        };

        // `ServerScoreboard.setDisplayObjective` (`ServerScoreboard.java:105-113`): an
        // objective that is not yet tracked is sent to clients here, before the display packet,
        // and only then recorded as tracked. An already-tracked one just gets the display packet.
        if let Some(name) = objective_name
            && !self.tracked_objectives.contains(name)
            && let Some(objective) = self.objectives.get(name)
        {
            let je_update = CUpdateObjectives::new(
                objective.name.clone(),
                Mode::Add,
                objective.display_name.clone(),
                objective.render_type,
                objective.number_format.clone(),
            );
            let be_update = BSetDisplayObjective {
                display_slot: slot_str.to_string(),
                objective_name: objective.name.clone(),
                display_name: objective.display_name.clone().get_text(),
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(0),
            };
            target.send_editioned(&je_update, &be_update).await;
            self.tracked_objectives.insert(name.to_string());
        }

        target.send_editioned(&je_display, &be_display).await;

        if let Some(name) = objective_name {
            self.display_slots.insert(slot, name.to_string());
        } else {
            self.display_slots.remove(&slot);
        }
    }

    pub async fn clear_display_objective(
        &mut self,
        target: &impl ScoreboardTarget,
        slot: ScoreboardDisplaySlot,
    ) {
        self.set_display_objective(target, slot, None).await;
    }

    pub async fn remove_objective(&mut self, target: &impl ScoreboardTarget, name: &str) {
        if !self.objectives.contains_key(name) {
            warn!(
                "Tried to remove an objective which does not exist: {}",
                name
            );
            return;
        }

        let je_packet = CUpdateObjectives::new(
            name.to_string(),
            Mode::Remove,
            TextComponent::empty(),
            RenderType::Integer,
            None,
        );

        let be_packet = BRemoveObjective {
            objective_name: name.to_string(),
        };

        target.send_editioned(&je_packet, &be_packet).await;

        if let Some(objective) = self.objectives.get(name)
            && let Some(objectives) = self.objectives_by_criterion.get_mut(&objective.criterion)
        {
            objectives.retain(|n| n != name);
            if objectives.is_empty() {
                let criterion = objective.criterion.clone();
                self.objectives_by_criterion.remove(&criterion);
            }
        }
        self.tracked_objectives.remove(name);
        self.objectives.remove(name);
        self.scores.remove(name);
        self.display_slots.retain(|_, obj| obj != name);
    }

    pub async fn update_score(
        &mut self,
        target: &impl ScoreboardTarget,
        mut score: ScoreboardScore,
    ) {
        if !self.objectives.contains_key(&score.objective_name) {
            warn!(
                "Tried to place a score into an objective which does not exist: {}",
                &score.objective_name
            );
            return;
        }

        // `Scoreboard#onScoreChanged`: when the objective has `display_auto_update` set, the
        // score's display name follows the score holder's display name.
        if let Some(objective) = self.objectives.get(&score.objective_name)
            && objective.display_auto_update
        {
            let entity_display = TextComponent::text(score.entity_name.clone());
            if score.display_name.as_ref() != Some(&entity_display) {
                score.display_name = Some(entity_display);
            }
        }

        let je_packet = CUpdateScore::new(
            score.entity_name.clone(),
            score.objective_name.clone(),
            score.value,
            score.display_name.clone(),
            score.number_format.clone(),
        );

        let be_packet = BSetScore {
            action: VarInt(0), // Change
            entries: vec![BScoreEntry {
                scoreboard_id: score.entity_name.as_ptr() as i64, // Internal ID
                objective_name: score.objective_name.clone(),
                score: score.value,
                entry_type: VarInt(3), // Fake player / Literal
                entity_unique_id: 0,
                custom_name: score.entity_name.clone(),
            }],
        };

        target.send_editioned(&je_packet, &be_packet).await;

        self.scores
            .entry(score.objective_name.clone())
            .or_default()
            .insert(score.entity_name.clone(), score);
    }

    pub async fn set_score_value(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: i32,
    ) {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let existing = self.get_score(&entity_s, &obj_s).cloned();
        let score = ScoreboardScore {
            entity_name: entity_s,
            objective_name: obj_s,
            value: VarInt(value),
            display_name: existing.as_ref().and_then(|s| s.display_name.clone()),
            number_format: existing.as_ref().and_then(|s| s.number_format.clone()),
            locked: existing.as_ref().is_none_or(|s| s.locked),
        };
        self.update_score(target, score).await;
    }

    pub async fn add_score(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        delta: i32,
    ) -> i32 {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let current_val = self.get_score_value(&entity_s, &obj_s).unwrap_or(0);
        let new_val = current_val + delta;
        self.set_score_value(target, entity_s, obj_s, new_val).await;
        new_val
    }

    pub async fn remove_score(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: &str,
        objective_name: &str,
    ) {
        let je_packet = CResetScore::new(entity_name.to_string(), Some(objective_name.to_string()));

        let be_packet = BSetScore {
            action: VarInt(1), // Remove
            entries: vec![BScoreEntry {
                scoreboard_id: entity_name.as_ptr() as i64,
                objective_name: objective_name.to_string(),
                score: VarInt(0),
                entry_type: VarInt(3),
                entity_unique_id: 0,
                custom_name: entity_name.to_string(),
            }],
        };

        target.send_editioned(&je_packet, &be_packet).await;

        if let Some(objective_scores) = self.scores.get_mut(objective_name) {
            objective_scores.remove(entity_name);
        }
    }

    pub async fn reset_scores_for_entity(
        &mut self,
        target: &impl ScoreboardTarget,
        entity_name: &str,
    ) {
        let je_packet = CResetScore::new(entity_name.to_string(), None);

        let mut be_entries = Vec::new();
        for (obj_name, obj_scores) in &self.scores {
            if obj_scores.contains_key(entity_name) {
                be_entries.push(BScoreEntry {
                    scoreboard_id: entity_name.as_ptr() as i64,
                    objective_name: obj_name.clone(),
                    score: VarInt(0),
                    entry_type: VarInt(3),
                    entity_unique_id: 0,
                    custom_name: entity_name.to_string(),
                });
            }
        }

        let be_packet = BSetScore {
            action: VarInt(1), // Remove
            entries: be_entries,
        };

        target.send_editioned(&je_packet, &be_packet).await;

        for obj_scores in self.scores.values_mut() {
            obj_scores.remove(entity_name);
        }
    }

    pub async fn add_team(&mut self, target: &impl ScoreboardTarget, team: Team) {
        if self.teams.contains_key(&team.name) {
            warn!("Tried to create Team which already exists, {}", team.name);
            return;
        }

        let parameters = TeamParameters {
            display_name: &team.display_name,
            options: team.options,
            nametag_visibility: team.nametag_visibility.to_str(),
            collision_rule: team.collision_rule.to_str(),
            color: team.color as i32,
            player_prefix: &team.player_prefix,
            player_suffix: &team.player_suffix,
        };

        target
            .send_je(&CSetPlayerTeam {
                team_name: team.name.clone(),
                method: TeamMethod::Create,
                parameters: Some(parameters),
                players: team.players.clone().into(),
            })
            .await;

        self.teams.insert(team.name.clone(), team);
    }

    pub async fn create_team(&mut self, target: &impl ScoreboardTarget, team: Team) {
        self.add_team(target, team).await;
    }

    pub async fn update_team(&mut self, target: &impl ScoreboardTarget, team: Team) {
        if !self.teams.contains_key(&team.name) {
            warn!("Tried to update Team which does not exist, {}", team.name);
            return;
        }

        let parameters = TeamParameters {
            display_name: &team.display_name,
            options: team.options,
            nametag_visibility: team.nametag_visibility.to_str(),
            collision_rule: team.collision_rule.to_str(),
            color: team.color as i32,
            player_prefix: &team.player_prefix,
            player_suffix: &team.player_suffix,
        };

        target
            .send_je(&CSetPlayerTeam {
                team_name: team.name.clone(),
                method: TeamMethod::Update,
                parameters: Some(parameters),
                players: Box::new([]),
            })
            .await;

        self.teams.insert(team.name.clone(), team);
    }

    pub async fn remove_team(&mut self, target: &impl ScoreboardTarget, name: &str) {
        if !self.teams.contains_key(name) {
            warn!("Tried to remove Team which does not exist, {}", name);
            return;
        }

        target
            .send_je(&CSetPlayerTeam {
                team_name: name.to_string(),
                method: TeamMethod::Remove,
                parameters: None,
                players: Box::new([]),
            })
            .await;

        self.teams.remove(name);
    }

    pub async fn add_player_to_team(
        &mut self,
        target: &impl ScoreboardTarget,
        team_name: &str,
        player: String,
    ) {
        let Some(team) = self.teams.get_mut(team_name) else {
            warn!(
                "Tried to add player to Team which does not exist, {}",
                team_name
            );
            return;
        };

        if team.players.contains(&player) {
            return;
        }

        target
            .send_je(&CSetPlayerTeam {
                team_name: team_name.to_string(),
                method: TeamMethod::AddPlayers,
                parameters: None,
                players: vec![player.clone()].into(),
            })
            .await;

        team.players.push(player);
    }

    pub async fn remove_player_from_team(
        &mut self,
        target: &impl ScoreboardTarget,
        team_name: &str,
        player: &str,
    ) {
        let Some(team) = self.teams.get_mut(team_name) else {
            warn!(
                "Tried to remove player from Team which does not exist, {}",
                team_name
            );
            return;
        };

        if !team.players.contains(&player.to_string()) {
            return;
        }

        target
            .send_je(&CSetPlayerTeam {
                team_name: team_name.to_string(),
                method: TeamMethod::RemovePlayers,
                parameters: None,
                players: vec![player.to_string()].into(),
            })
            .await;

        team.players.retain(|p| p != player);
    }

    pub async fn clear_team_players(&mut self, target: &impl ScoreboardTarget, team_name: &str) {
        let Some(team) = self.teams.get_mut(team_name) else {
            warn!(
                "Tried to clear players from Team which does not exist, {}",
                team_name
            );
            return;
        };

        if team.players.is_empty() {
            return;
        }

        let players_to_remove = team.players.clone();
        target
            .send_je(&CSetPlayerTeam {
                team_name: team_name.to_string(),
                method: TeamMethod::RemovePlayers,
                parameters: None,
                players: players_to_remove.into(),
            })
            .await;

        team.players.clear();
    }

    pub async fn send_to_player(&self, player: &Player) {
        for objective in self.objectives.values() {
            let je_update = CUpdateObjectives::new(
                objective.name.clone(),
                Mode::Add,
                objective.display_name.clone(),
                objective.render_type,
                objective.number_format.clone(),
            );
            let be_update = BSetDisplayObjective {
                display_slot: "sidebar".to_string(),
                objective_name: objective.name.clone(),
                display_name: objective.display_name.clone().get_text(),
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(0),
            };
            player.send_editioned(&je_update, &be_update).await;
        }

        for (slot, objective_name) in &self.display_slots {
            let slot_str = match slot {
                ScoreboardDisplaySlot::List => "list",
                ScoreboardDisplaySlot::BelowName => "belowname",
                _ => "sidebar",
            };
            let display_name = self.objectives.get(objective_name).map_or_else(
                || objective_name.clone(),
                |o| o.display_name.clone().get_text(),
            );
            let je_display = CDisplayObjective::new(*slot, objective_name.clone());
            let be_display = BSetDisplayObjective {
                display_slot: slot_str.to_string(),
                objective_name: objective_name.clone(),
                display_name,
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(0),
            };
            player.send_editioned(&je_display, &be_display).await;
        }

        for objective_scores in self.scores.values() {
            for score in objective_scores.values() {
                let je_packet = CUpdateScore::new(
                    score.entity_name.clone(),
                    score.objective_name.clone(),
                    score.value,
                    score.display_name.clone(),
                    score.number_format.clone(),
                );
                let be_packet = BSetScore {
                    action: VarInt(0),
                    entries: vec![BScoreEntry {
                        scoreboard_id: score.entity_name.as_ptr() as i64,
                        objective_name: score.objective_name.clone(),
                        score: score.value,
                        entry_type: VarInt(3),
                        entity_unique_id: 0,
                        custom_name: score.entity_name.clone(),
                    }],
                };
                player.send_editioned(&je_packet, &be_packet).await;
            }
        }

        for team in self.teams.values() {
            let parameters = TeamParameters {
                display_name: &team.display_name,
                options: team.options,
                nametag_visibility: team.nametag_visibility.to_str(),
                collision_rule: team.collision_rule.to_str(),
                color: team.color as i32,
                player_prefix: &team.player_prefix,
                player_suffix: &team.player_suffix,
            };
            let je_packet = CSetPlayerTeam {
                team_name: team.name.clone(),
                method: TeamMethod::Create,
                parameters: Some(parameters),
                players: team.players.clone().into(),
            };
            player.send_client_packet(&je_packet).await;
        }
    }

    /// Finds the team a scoreholder (player name, or UUID string for non-player entities)
    /// belongs to. Mirrors `Scoreboard.getPlayersTeam` (`net.minecraft.world.scores.Scoreboard`).
    #[must_use]
    pub fn get_team_for_scoreboard_name(&self, name: &str) -> Option<&Team> {
        self.teams
            .values()
            .find(|team| team.players.iter().any(|p| p == name))
    }

    /// Updates display name, render type and number format on an existing objective and
    /// sends the update packet to all players if the objective is tracked.
    pub fn modify_objective(
        &mut self,
        world: &World,
        objective_name: &str,
        display_name: TextComponent,
        render_type: RenderType,
        number_format: Option<NumberFormat>,
    ) -> bool {
        let Some(objective) = self.objectives.get_mut(objective_name) else {
            return false;
        };
        objective.display_name = display_name.clone();
        objective.render_type = render_type;
        objective.number_format.clone_from(&number_format);

        if self.tracked_objectives.contains(objective_name) {
            let je_update = CUpdateObjectives::new(
                objective_name.to_string(),
                Mode::Update,
                display_name,
                render_type,
                number_format,
            );
            world.broadcast_packet_all(&je_update);
        }
        true
    }

    /// Sets the `display_auto_update` flag on an objective. When true, the score's
    /// display name is automatically updated to the score holder's display name
    /// whenever the score is modified.
    pub fn set_display_auto_update(
        &mut self,
        world: &World,
        objective_name: &str,
        display_auto_update: bool,
    ) -> bool {
        let Some(objective) = self.objectives.get_mut(objective_name) else {
            return false;
        };
        if objective.display_auto_update == display_auto_update {
            return true;
        }
        objective.display_auto_update = display_auto_update;

        if self.tracked_objectives.contains(objective_name) {
            let je_update = CUpdateObjectives::new(
                objective_name.to_string(),
                Mode::Update,
                objective.display_name.clone(),
                objective.render_type,
                objective.number_format.clone(),
            );
            world.broadcast_packet_all(&je_update);
        }
        true
    }

    pub fn set_objective_number_format(
        &mut self,
        world: &World,
        objective_name: &str,
        number_format: Option<NumberFormat>,
    ) -> bool {
        let Some(objective) = self.objectives.get_mut(objective_name) else {
            return false;
        };
        objective.number_format.clone_from(&number_format);

        if self.tracked_objectives.contains(objective_name) {
            let je_update = CUpdateObjectives::new(
                objective_name.to_string(),
                Mode::Update,
                objective.display_name.clone(),
                objective.render_type,
                number_format,
            );
            world.broadcast_packet_all(&je_update);
        }
        true
    }

    /// Updates all objectives that use the given criterion with the specified value for the
    /// entity. Automatically creates score entries if they don't exist.
    pub async fn for_all_objectives(
        &mut self,
        world: &World,
        criterion: &str,
        entity_name: &str,
        value: i32,
    ) {
        let Some(objective_names) = self.objectives_by_criterion.get(criterion).cloned() else {
            return;
        };

        for objective_name in &objective_names {
            let score = ScoreboardScore {
                entity_name: entity_name.to_string(),
                objective_name: objective_name.clone(),
                value: VarInt(value),
                display_name: None,
                number_format: None,
                locked: false,
            };
            self.update_score(world, score).await;
        }
    }

    /// Returns whether the given objective is currently tracked (known to clients).
    #[must_use]
    pub fn is_tracked(&self, objective_name: &str) -> bool {
        self.tracked_objectives.contains(objective_name)
    }

    /// Returns all score holders that have at least one score entry.
    #[must_use]
    pub fn get_tracked_players(&self) -> Vec<&str> {
        let mut players: Vec<&str> = self
            .scores
            .values()
            .flat_map(|scores| scores.keys().map(String::as_str))
            .collect();
        players.sort_unstable();
        players.dedup();
        players
    }

    /// Resets a single score for a player. Sends a reset-score packet if the
    /// objective was tracked.
    pub fn reset_single_player_score(
        &mut self,
        world: &World,
        entity_name: &str,
        objective_name: &str,
    ) {
        let was_removed = self
            .scores
            .get_mut(objective_name)
            .is_some_and(|m| m.remove(entity_name).is_some());

        if was_removed && self.tracked_objectives.contains(objective_name) {
            let packet =
                CResetScore::new(entity_name.to_string(), Some(objective_name.to_string()));
            world.broadcast_packet_all(&packet);
        }
    }

    /// Resets all scores for a player across all objectives.
    pub fn reset_all_player_scores(&mut self, world: &World, entity_name: &str) {
        let mut any_removed = false;
        let tracked = &self.tracked_objectives;
        self.scores.retain(|objective_name, scores| {
            if scores.remove(entity_name).is_some() {
                any_removed = true;
                if tracked.contains(objective_name) {
                    let packet =
                        CResetScore::new(entity_name.to_string(), Some(objective_name.clone()));
                    world.broadcast_packet_all(&packet);
                }
            }
            !scores.is_empty()
        });

        if any_removed {
            let packet = CResetScore::new(entity_name.to_string(), None);
            world.broadcast_packet_all(&packet);
        }
    }

    #[must_use]
    pub fn get_player_score_info(
        &self,
        entity_name: &str,
        objective_name: &str,
    ) -> Option<&ScoreboardScore> {
        self.scores
            .get(objective_name)
            .and_then(|m| m.get(entity_name))
    }

    pub fn set_score_number_format(
        &mut self,
        world: &World,
        entity_name: &str,
        objective_name: &str,
        number_format: Option<NumberFormat>,
    ) -> bool {
        let tracked = self.tracked_objectives.contains(objective_name);
        let Some(score) = self
            .scores
            .get_mut(objective_name)
            .and_then(|m| m.get_mut(entity_name))
        else {
            return false;
        };
        score.number_format.clone_from(&number_format);
        if tracked {
            let packet = CUpdateScore::new(
                entity_name.to_string(),
                objective_name.to_string(),
                score.value,
                score.display_name.clone(),
                number_format,
            );
            world.broadcast_packet_all(&packet);
        }
        true
    }

    /// Sets the optional display name for a score entry. Sends packet if tracked.
    pub fn set_score_display_name(
        &mut self,
        world: &World,
        entity_name: &str,
        objective_name: &str,
        display_name: Option<TextComponent>,
    ) -> bool {
        let tracked = self.tracked_objectives.contains(objective_name);
        let Some(score) = self
            .scores
            .get_mut(objective_name)
            .and_then(|m| m.get_mut(entity_name))
        else {
            return false;
        };
        score.display_name.clone_from(&display_name);
        if tracked {
            let packet = CUpdateScore::new(
                entity_name.to_string(),
                objective_name.to_string(),
                score.value,
                display_name,
                score.number_format.clone(),
            );
            world.broadcast_packet_all(&packet);
        }
        true
    }

    #[must_use]
    pub fn list_scores_for_objective(&self, objective_name: &str) -> Vec<&ScoreboardScore> {
        self.scores
            .get(objective_name)
            .map(|m| m.values().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn list_scores_for_player(&self, entity_name: &str) -> Vec<(&str, i32)> {
        let mut result = Vec::new();
        for (obj_name, scores) in &self.scores {
            if let Some(score) = scores.get(entity_name) {
                result.push((obj_name.as_str(), score.value.0));
            }
        }
        result
    }

    /// Sends the full scoreboard state (all tracked objectives + display slots + scores)
    /// to a single player. Called when a player joins the world.
    pub fn send_state_to_player(&self, player: &Player) {
        let ClientPlatform::Java(client) = player.client.as_ref() else {
            return;
        };
        for objective_name in &self.tracked_objectives {
            let Some(objective) = self.objectives.get(objective_name.as_str()) else {
                continue;
            };
            let je_create = CUpdateObjectives::new(
                objective_name.clone(),
                Mode::Add,
                objective.display_name.clone(),
                objective.render_type,
                objective.number_format.clone(),
            );
            // FIXME: using try_enqueue_packet since this is called in spawn context
            if let Ok(data) = client.serialize_packet(&je_create) {
                client.try_enqueue_packet(data);
            }

            if let Some(objective_scores) = self.scores.get(objective_name.as_str()) {
                for score in objective_scores.values() {
                    let je_score = CUpdateScore::new(
                        score.entity_name.clone(),
                        score.objective_name.clone(),
                        score.value,
                        score.display_name.clone(),
                        score.number_format.clone(),
                    );
                    if let Ok(data) = client.serialize_packet(&je_score) {
                        client.try_enqueue_packet(data);
                    }
                }
            }
        }

        for (slot, objective_name) in &self.display_slots {
            let je_display = CDisplayObjective::new(*slot, objective_name.clone());
            if let Ok(data) = client.serialize_packet(&je_display) {
                client.try_enqueue_packet(data);
            }
        }
    }

    /// Converts the in-memory scoreboard to a serializable data structure for disk
    /// storage, matching vanilla's `ScoreboardSaveData.Packed` field layout exactly
    /// (see `net.minecraft.world.scores.ScoreboardSaveData`, decompiled 26.2 source)
    /// so the resulting `scoreboard.dat` is readable by vanilla and vice versa.
    #[must_use]
    pub fn to_data(&self) -> pumpkin_world::world_info::data_files::ScoreboardData {
        use pumpkin_world::world_info::data_files::{
            SerializableObjective, SerializableScore, SerializableTeam,
        };

        let objectives = self
            .objectives
            .values()
            .map(|obj| SerializableObjective {
                name: obj.name.clone(),
                display_name: obj.display_name.clone().0,
                render_type: match obj.render_type {
                    RenderType::Integer => "integer".to_string(),
                    RenderType::Hearts => "hearts".to_string(),
                },
                criteria_name: obj.criterion.clone(),
                display_auto_update: obj.display_auto_update,
                number_format: obj
                    .number_format
                    .as_ref()
                    .map(|nf| serde_json::to_string(nf).unwrap_or_default()),
            })
            .collect();

        let mut scores = Vec::new();
        for (obj_name, obj_scores) in &self.scores {
            for score in obj_scores.values() {
                scores.push(SerializableScore {
                    entity_name: score.entity_name.clone(),
                    objective_name: obj_name.clone(),
                    value: score.value.0,
                    locked: score.locked,
                    display: score.display_name.clone().map(|d| d.0),
                    number_format: score
                        .number_format
                        .as_ref()
                        .map(|nf| serde_json::to_string(nf).unwrap_or_default()),
                });
            }
        }

        let teams = self
            .teams
            .values()
            .map(|t| SerializableTeam {
                name: t.name.clone(),
                display_name: Some(t.display_name.clone().0),
                color: Some(named_color_to_str(t.color).to_string()),
                friendly_fire: t.options & 0x01 != 0,
                see_friendly_invisibles: t.options & 0x02 != 0,
                player_prefix: t.player_prefix.clone().0,
                player_suffix: t.player_suffix.clone().0,
                nametag_visibility: t.nametag_visibility.to_str().to_string(),
                death_message_visibility: t.death_message_visibility.to_str().to_string(),
                collision_rule: t.collision_rule.to_str().to_string(),
                players: t.players.clone(),
            })
            .collect();

        let display_slots = self
            .display_slots
            .iter()
            .map(|(slot, name)| (display_slot_name(*slot).to_string(), name.clone()))
            .collect();

        pumpkin_world::world_info::data_files::ScoreboardData {
            objectives,
            scores,
            teams,
            display_slots,
        }
    }

    /// Populates this scoreboard from serialized data (loaded from disk).
    pub fn load_from_data(&mut self, data: &pumpkin_world::world_info::data_files::ScoreboardData) {
        self.objectives.clear();
        self.teams.clear();
        self.scores.clear();
        self.display_slots.clear();
        self.tracked_objectives.clear();
        self.objectives_by_criterion.clear();

        for obj in &data.objectives {
            let render_type = if obj.render_type == "hearts" {
                RenderType::Hearts
            } else {
                RenderType::Integer
            };
            let number_format = obj
                .number_format
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let mut objective = ScoreboardObjective::new(
                obj.name.clone(),
                TextComponent(obj.display_name.clone()),
                render_type,
                number_format,
                obj.criteria_name.clone(),
            );
            objective.display_auto_update = obj.display_auto_update;
            self.objectives_by_criterion
                .entry(obj.criteria_name.clone())
                .or_default()
                .push(obj.name.clone());
            self.objectives.insert(obj.name.clone(), objective);
            self.tracked_objectives.insert(obj.name.clone());
        }

        for score in &data.scores {
            let number_format = score
                .number_format
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let entry = self.scores.entry(score.objective_name.clone()).or_default();
            entry.insert(
                score.entity_name.clone(),
                ScoreboardScore {
                    entity_name: score.entity_name.clone(),
                    objective_name: score.objective_name.clone(),
                    value: VarInt(score.value),
                    display_name: score.display.clone().map(TextComponent),
                    number_format,
                    locked: score.locked,
                },
            );
        }

        for team in &data.teams {
            let display_name = team
                .display_name
                .clone()
                .map_or_else(TextComponent::empty, TextComponent);
            let color = team
                .color
                .as_deref()
                .and_then(|c| NamedColor::try_from(c).ok())
                .unwrap_or(NamedColor::White);
            let mut options = 0i8;
            if team.friendly_fire {
                options |= 0x01;
            }
            if team.see_friendly_invisibles {
                options |= 0x02;
            }
            self.teams.insert(
                team.name.clone(),
                Team {
                    name: team.name.clone(),
                    display_name,
                    options,
                    nametag_visibility: NameTagVisibility::parse(&team.nametag_visibility),
                    death_message_visibility: NameTagVisibility::parse(
                        &team.death_message_visibility,
                    ),
                    collision_rule: CollisionRule::parse(&team.collision_rule),
                    color,
                    player_prefix: TextComponent(team.player_prefix.clone()),
                    player_suffix: TextComponent(team.player_suffix.clone()),
                    players: team.players.clone(),
                },
            );
        }

        for (slot_name, obj_name) in &data.display_slots {
            if let Some(slot) = display_slot_from_name(slot_name) {
                self.display_slots.insert(slot, obj_name.clone());
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardObjective {
    pub name: String,
    pub display_name: TextComponent,
    pub render_type: RenderType,
    pub number_format: Option<NumberFormat>,
    pub criterion: String,
    /// `Objective#displayAutoUpdate`: when set, a score's display name follows the score
    /// holder's display name.
    pub display_auto_update: bool,
}

impl ScoreboardObjective {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        display_name: TextComponent,
        render_type: RenderType,
        number_format: Option<NumberFormat>,
        criterion: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            display_name,
            render_type,
            number_format,
            criterion: criterion.into(),
            display_auto_update: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardScore {
    pub entity_name: String,
    pub objective_name: String,
    pub value: VarInt,
    pub display_name: Option<TextComponent>,
    pub number_format: Option<NumberFormat>,
    pub locked: bool,
}

impl ScoreboardScore {
    #[must_use]
    pub fn new(
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: VarInt,
        display_name: Option<TextComponent>,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            entity_name: entity_name.into(),
            objective_name: objective_name.into(),
            value,
            display_name,
            number_format,
            locked: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameTagVisibility {
    Always,
    Never,
    HideForOtherTeams,
    HideForOwnTeam,
}

impl NameTagVisibility {
    #[must_use]
    pub const fn to_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::HideForOtherTeams => "hideForOtherTeams",
            Self::HideForOwnTeam => "hideForOwnTeam",
        }
    }

    /// Inverse of [`Self::to_str`]. Unknown values fall back to `Always`, matching
    /// vanilla's `Team.Visibility.CODEC` behavior of defaulting on decode failure.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "never" => Self::Never,
            "hideForOtherTeams" => Self::HideForOtherTeams,
            "hideForOwnTeam" => Self::HideForOwnTeam,
            _ => Self::Always,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionRule {
    Always,
    Never,
    PushOtherTeams,
    PushOwnTeam,
}

impl CollisionRule {
    #[must_use]
    pub const fn to_str(&self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::PushOtherTeams => "pushOtherTeams",
            Self::PushOwnTeam => "pushOwnTeam",
        }
    }

    /// Inverse of [`Self::to_str`]. Unknown values fall back to `Always`, matching
    /// vanilla's `Team.CollisionRule.CODEC` behavior of defaulting on decode failure.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "never" => Self::Never,
            "pushOtherTeams" => Self::PushOtherTeams,
            "pushOwnTeam" => Self::PushOwnTeam,
            _ => Self::Always,
        }
    }
}

/// A scoreholder's scoreboard entry name: the player name for players, or the UUID string for
/// any other entity (`net.minecraft.world.entity.Entity#getScoreboardName`).
#[must_use]
pub fn entity_scoreboard_name(entity: &dyn crate::entity::EntityBase) -> String {
    // Vanilla uses the entity's `getScoreboardName` for non-player scoreholders
    // (`Entity.java:3259-3262`).
    entity.get_player().map_or_else(
        || entity.get_entity().get_scoreboard_name(),
        |player| player.gameprofile.name.clone(),
    )
}

/// vanilla `EntitySelector.pushableBy(Entity)`.
///
/// Whether `pusher` and `other` should push each other given their resolved collision rules
/// (`Team.CollisionRule::ALWAYS` when a side has no team) and whether they share a team.
/// (net.minecraft.world.entity.EntitySelector, decompiled 26.2, lines 29-56)
#[must_use]
pub const fn collision_rule_permits_push(
    pusher: CollisionRule,
    other: CollisionRule,
    same_team: bool,
) -> bool {
    if matches!(pusher, CollisionRule::Never) || matches!(other, CollisionRule::Never) {
        return false;
    }
    if (matches!(pusher, CollisionRule::PushOwnTeam) || matches!(other, CollisionRule::PushOwnTeam))
        && same_team
    {
        return false;
    }
    (!matches!(pusher, CollisionRule::PushOtherTeams)
        && !matches!(other, CollisionRule::PushOtherTeams))
        || same_team
}

#[cfg(test)]
mod collision_rule_tests {
    use super::CollisionRule::{Always, Never, PushOtherTeams, PushOwnTeam};
    use super::collision_rule_permits_push;

    #[test]
    fn no_team_always_pushes() {
        assert!(collision_rule_permits_push(Always, Always, false));
    }

    #[test]
    fn never_blocks_regardless_of_side_or_team() {
        assert!(!collision_rule_permits_push(Never, Always, false));
        assert!(!collision_rule_permits_push(Always, Never, false));
        assert!(!collision_rule_permits_push(Never, Never, true));
    }

    #[test]
    fn push_own_team_blocks_only_within_the_same_team() {
        assert!(!collision_rule_permits_push(PushOwnTeam, Always, true));
        assert!(collision_rule_permits_push(PushOwnTeam, Always, false));
    }

    #[test]
    fn push_other_teams_blocks_across_teams_but_allows_within_same_team() {
        assert!(!collision_rule_permits_push(PushOtherTeams, Always, false));
        assert!(collision_rule_permits_push(PushOtherTeams, Always, true));
    }

    #[test]
    fn push_own_team_wins_over_push_other_teams_on_same_team() {
        assert!(!collision_rule_permits_push(
            PushOwnTeam,
            PushOtherTeams,
            true
        ));
    }
}

#[derive(Clone, Debug)]
pub struct Team {
    pub name: String,
    pub display_name: TextComponent,
    pub options: i8,
    pub nametag_visibility: NameTagVisibility,
    /// Vanilla also tracks this separately from `nametag_visibility`
    /// (`PlayerTeam.Packed#deathMessageVisibility`); Pumpkin does not yet act on it
    /// for gameplay, but it is preserved across `scoreboard.dat` round-trips.
    pub death_message_visibility: NameTagVisibility,
    pub collision_rule: CollisionRule,
    pub color: NamedColor,
    pub player_prefix: TextComponent,
    pub player_suffix: TextComponent,
    pub players: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BedrockSortOrder {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BedrockDisplaySlot {
    PlayerList,
    Sidebar,
    BelowName,
}

impl BedrockDisplaySlot {
    #[must_use]
    pub const fn to_str(&self) -> &'static str {
        match self {
            Self::PlayerList => "list",
            Self::Sidebar => "sidebar",
            Self::BelowName => "belowname",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BedrockObjective {
    pub name: String,
    pub display_name: String,
    pub sort_order: BedrockSortOrder,
}

#[derive(Clone, Debug, Default)]
pub struct BedrockScoreboard {
    pub objectives: HashMap<String, BedrockObjective>,
    pub display_slots: HashMap<BedrockDisplaySlot, String>,
    pub scores: HashMap<(String, String), i32>,
}

impl BedrockScoreboard {
    pub async fn add_objective(&mut self, player: &Player, objective: BedrockObjective) {
        let be_update = BSetDisplayObjective {
            display_slot: "sidebar".to_string(),
            objective_name: objective.name.clone(),
            display_name: objective.display_name.clone(),
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(match objective.sort_order {
                BedrockSortOrder::Ascending => 0,
                BedrockSortOrder::Descending => 1,
            }),
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                    objective.name.clone(),
                    pumpkin_protocol::java::client::play::Mode::Add,
                    TextComponent::text(objective.display_name.clone()),
                    pumpkin_protocol::java::client::play::RenderType::Integer,
                    None,
                ),
                &be_update,
            )
            .await;

        self.objectives.insert(objective.name.clone(), objective);
    }

    pub async fn update_objective(&mut self, player: &Player, objective: BedrockObjective) {
        let be_update = BSetDisplayObjective {
            display_slot: "sidebar".to_string(),
            objective_name: objective.name.clone(),
            display_name: objective.display_name.clone(),
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(match objective.sort_order {
                BedrockSortOrder::Ascending => 0,
                BedrockSortOrder::Descending => 1,
            }),
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                    objective.name.clone(),
                    pumpkin_protocol::java::client::play::Mode::Update,
                    TextComponent::text(objective.display_name.clone()),
                    pumpkin_protocol::java::client::play::RenderType::Integer,
                    None,
                ),
                &be_update,
            )
            .await;

        self.objectives.insert(objective.name.clone(), objective);
    }

    pub async fn remove_objective(&mut self, player: &Player, name: &str) {
        let be_remove = BRemoveObjective {
            objective_name: name.to_string(),
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                    name.to_string(),
                    pumpkin_protocol::java::client::play::Mode::Remove,
                    TextComponent::text(""),
                    pumpkin_protocol::java::client::play::RenderType::Integer,
                    None,
                ),
                &be_remove,
            )
            .await;
        self.objectives.remove(name);
        self.display_slots.retain(|_, v| v != name);
        self.scores.retain(|(_, obj), _| obj != name);
    }

    pub async fn set_display_objective(
        &mut self,
        player: &Player,
        slot: BedrockDisplaySlot,
        objective_name: Option<&str>,
    ) {
        let obj_name_str = objective_name.unwrap_or("");
        let display_name = objective_name
            .and_then(|name| self.objectives.get(name))
            .map_or_else(|| obj_name_str.to_string(), |o| o.display_name.clone());

        let be_display = BSetDisplayObjective {
            display_slot: slot.to_str().to_string(),
            objective_name: obj_name_str.to_string(),
            display_name,
            criteria_name: "dummy".to_string(),
            sort_order: VarInt(0),
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CDisplayObjective::new(
                    match slot {
                        BedrockDisplaySlot::PlayerList => ScoreboardDisplaySlot::List,
                        BedrockDisplaySlot::Sidebar => ScoreboardDisplaySlot::Sidebar,
                        BedrockDisplaySlot::BelowName => ScoreboardDisplaySlot::BelowName,
                    },
                    obj_name_str.to_string(),
                ),
                &be_display,
            )
            .await;

        if let Some(name) = objective_name {
            self.display_slots.insert(slot, name.to_string());
        } else {
            self.display_slots.remove(&slot);
        }
    }

    pub async fn clear_display_objective(&mut self, player: &Player, slot: BedrockDisplaySlot) {
        self.set_display_objective(player, slot, None).await;
    }

    pub async fn update_score(
        &mut self,
        player: &Player,
        entity_name: &str,
        objective_name: &str,
        value: i32,
    ) {
        let score = ScoreboardScore::new(
            entity_name.to_string(),
            objective_name.to_string(),
            VarInt(value),
            None,
            None,
        );
        let be_score = BSetScore {
            action: VarInt(0),
            entries: vec![],
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CUpdateScore::new(
                    score.entity_name.clone(),
                    score.objective_name.clone(),
                    score.value,
                    score.display_name.clone(),
                    score.number_format.clone(),
                ),
                &be_score,
            )
            .await;

        self.scores
            .insert((entity_name.to_string(), objective_name.to_string()), value);
    }

    pub async fn add_score(
        &mut self,
        player: &Player,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        delta: i32,
    ) -> i32 {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let current = self
            .scores
            .get(&(entity_s.clone(), obj_s.clone()))
            .copied()
            .unwrap_or(0);
        let new_val = current + delta;
        self.update_score(player, &entity_s, &obj_s, new_val).await;
        new_val
    }

    pub async fn remove_score(&mut self, player: &Player, entity_name: &str, objective_name: &str) {
        let be_score = BSetScore {
            action: VarInt(1),
            entries: vec![],
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CResetScore::new(
                    entity_name.to_string(),
                    Some(objective_name.to_string()),
                ),
                &be_score,
            )
            .await;
        self.scores
            .remove(&(entity_name.to_string(), objective_name.to_string()));
    }

    pub async fn reset_scores_for_entity(&mut self, player: &Player, entity_name: &str) {
        let be_score = BSetScore {
            action: VarInt(1),
            entries: vec![],
        };
        player
            .send_editioned(
                &pumpkin_protocol::java::client::play::CResetScore::new(
                    entity_name.to_string(),
                    None,
                ),
                &be_score,
            )
            .await;
        self.scores.retain(|(entity, _), _| entity != entity_name);
    }

    pub async fn send_to_player(&self, player: &Player) {
        for objective in self.objectives.values() {
            let be_update = BSetDisplayObjective {
                display_slot: "sidebar".to_string(),
                objective_name: objective.name.clone(),
                display_name: objective.display_name.clone(),
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(match objective.sort_order {
                    BedrockSortOrder::Ascending => 0,
                    BedrockSortOrder::Descending => 1,
                }),
            };
            player
                .send_editioned(
                    &pumpkin_protocol::java::client::play::CUpdateObjectives::new(
                        objective.name.clone(),
                        pumpkin_protocol::java::client::play::Mode::Add,
                        TextComponent::text(objective.display_name.clone()),
                        pumpkin_protocol::java::client::play::RenderType::Integer,
                        None,
                    ),
                    &be_update,
                )
                .await;
        }

        for (slot, objective_name) in &self.display_slots {
            let display_name = self
                .objectives
                .get(objective_name)
                .map_or_else(|| objective_name.clone(), |o| o.display_name.clone());
            let be_display = BSetDisplayObjective {
                display_slot: slot.to_str().to_string(),
                objective_name: objective_name.clone(),
                display_name,
                criteria_name: "dummy".to_string(),
                sort_order: VarInt(0),
            };
            player
                .send_editioned(
                    &pumpkin_protocol::java::client::play::CDisplayObjective::new(
                        match slot {
                            BedrockDisplaySlot::PlayerList => ScoreboardDisplaySlot::List,
                            BedrockDisplaySlot::Sidebar => ScoreboardDisplaySlot::Sidebar,
                            BedrockDisplaySlot::BelowName => ScoreboardDisplaySlot::BelowName,
                        },
                        objective_name.clone(),
                    ),
                    &be_display,
                )
                .await;
        }

        for ((entity_name, objective_name), value) in &self.scores {
            let score = ScoreboardScore::new(
                entity_name.clone(),
                objective_name.clone(),
                VarInt(*value),
                None,
                None,
            );
            let be_score = BSetScore {
                action: VarInt(0),
                entries: vec![],
            };
            player
                .send_editioned(
                    &pumpkin_protocol::java::client::play::CUpdateScore::new(
                        score.entity_name,
                        score.objective_name,
                        score.value,
                        score.display_name,
                        score.number_format,
                    ),
                    &be_score,
                )
                .await;
        }
    }
}

#[derive(Default, Debug)]
pub struct ScoreboardBuilder {
    scoreboard: Scoreboard,
}

impl ScoreboardBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn objective(mut self, name: impl Into<String>, display_name: TextComponent) -> Self {
        let obj = ScoreboardObjective::new(
            name.into(),
            display_name,
            RenderType::Integer,
            None,
            "dummy",
        );
        self.scoreboard.objectives.insert(obj.name.clone(), obj);
        self
    }

    #[must_use]
    pub fn objective_with_render(
        mut self,
        name: impl Into<String>,
        display_name: TextComponent,
        render_type: RenderType,
    ) -> Self {
        let obj = ScoreboardObjective::new(name.into(), display_name, render_type, None, "dummy");
        self.scoreboard.objectives.insert(obj.name.clone(), obj);
        self
    }

    #[must_use]
    pub fn display_slot(
        mut self,
        slot: ScoreboardDisplaySlot,
        objective_name: impl Into<String>,
    ) -> Self {
        self.scoreboard
            .display_slots
            .insert(slot, objective_name.into());
        self
    }

    #[must_use]
    pub fn score(
        mut self,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: i32,
    ) -> Self {
        let entity_s = entity_name.into();
        let obj_s = objective_name.into();
        let score =
            ScoreboardScore::new(entity_s.clone(), obj_s.clone(), VarInt(value), None, None);
        self.scoreboard
            .scores
            .entry(entity_s)
            .or_default()
            .insert(obj_s, score);
        self
    }

    #[must_use]
    pub fn team(mut self, team: Team) -> Self {
        self.scoreboard.teams.insert(team.name.clone(), team);
        self
    }

    #[must_use]
    pub fn build(self) -> Scoreboard {
        self.scoreboard
    }
}

#[derive(Default, Debug)]
pub struct BedrockScoreboardBuilder {
    scoreboard: BedrockScoreboard,
}

impl BedrockScoreboardBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn objective(
        mut self,
        name: impl Into<String>,
        display_name: impl Into<String>,
        sort_order: BedrockSortOrder,
    ) -> Self {
        let name = name.into();
        let obj = BedrockObjective {
            name: name.clone(),
            display_name: display_name.into(),
            sort_order,
        };
        self.scoreboard.objectives.insert(name, obj);
        self
    }

    #[must_use]
    pub fn display_slot(
        mut self,
        slot: BedrockDisplaySlot,
        objective_name: impl Into<String>,
    ) -> Self {
        self.scoreboard
            .display_slots
            .insert(slot, objective_name.into());
        self
    }

    #[must_use]
    pub fn score(
        mut self,
        entity_name: impl Into<String>,
        objective_name: impl Into<String>,
        value: i32,
    ) -> Self {
        self.scoreboard
            .scores
            .insert((entity_name.into(), objective_name.into()), value);
        self
    }

    #[must_use]
    pub fn build(self) -> BedrockScoreboard {
        self.scoreboard
    }
}
