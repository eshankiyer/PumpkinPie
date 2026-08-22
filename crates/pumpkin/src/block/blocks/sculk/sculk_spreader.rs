//! `SculkSpreader` port (`net/minecraft/world/level/block/SculkSpreader.java`, 360 lines).
//!
//! This is the piece the earlier steps deliberately left out: the cursor list itself, its
//! per-tick `update`, movement between sculk-behaviour blocks, merging, the 32-cursor cap
//! and NBT persistence. `sculk_behaviour.rs` already holds `ChargeCursor` and
//! `SculkSpreaderConfig` (vanilla keeps both inside `SculkSpreader`); this module owns the
//! list and the driving loop.

use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, RandomImpl};
use pumpkin_world::world::BlockAccessor;

use crate::block::blocks::abstract_multiface::FaceSet;
use crate::block::blocks::sculk::SculkBlock;
use crate::block::blocks::sculk_vein::{self, SculkVeinBlock};
use crate::block::sculk_behaviour::{
    ChargeCursor, DefaultSculkBehaviour, SculkBehaviour, SculkSpreaderConfig, SculkWorld,
};

/// `SculkSpreader.MAX_GROWTH_RATE_RADIUS` (`SculkSpreader.java:37`).
pub const MAX_GROWTH_RATE_RADIUS: i32 = 24;
/// `SculkSpreader.MAX_CHARGE` (`SculkSpreader.java:38`).
pub const MAX_CHARGE: i32 = 1000;
/// `SculkSpreader.MAX_CURSORS` (`SculkSpreader.java:40`).
pub const MAX_CURSORS: usize = 32;
/// `SculkSpreader.SHRIEKER_PLACEMENT_RATE` (`SculkSpreader.java:41`).
pub const SHRIEKER_PLACEMENT_RATE: i32 = 11;

static SCULK_BEHAVIOUR: SculkBlock = SculkBlock;
static SCULK_VEIN_BEHAVIOUR: SculkVeinBlock = SculkVeinBlock;
static DEFAULT_BEHAVIOUR: DefaultSculkBehaviour = DefaultSculkBehaviour;

/// `ChargeCursor.getBlockBehaviour` (`SculkSpreader.java:305-307`).
///
/// Verified against the decompile: exactly two blocks implement `SculkBehaviour`
/// (`SculkBlock.java`, `SculkVeinBlock.java`); everything else falls back to
/// `SculkBehaviour.DEFAULT`.
#[must_use]
pub fn behaviour_for(block: &Block) -> &'static dyn SculkBehaviour {
    match block.id {
        BlockId::SCULK => &SCULK_BEHAVIOUR,
        BlockId::SCULK_VEIN => &SCULK_VEIN_BEHAVIOUR,
        _ => &DEFAULT_BEHAVIOUR,
    }
}

/// `state.getBlock() instanceof SculkBehaviour`, the gate `getValidMovementPos` and the
/// `facings` refresh use.
#[must_use]
pub const fn is_sculk_behaviour_block(block: &Block) -> bool {
    matches!(block.id, BlockId::SCULK | BlockId::SCULK_VEIN)
}

/// `MultifaceBlock.availableFaces(state)`: the empty set for a non-multiface block, and
/// the vein's own face bits for `sculk_vein` (the only `SculkBehaviour` multiface block).
#[must_use]
fn available_faces(state: &BlockState) -> FaceSet {
    sculk_vein::existing_vein_faces(state).unwrap_or(FaceSet::EMPTY)
}

/// `ChargeCursor.NON_CORNER_NEIGHBOURS` (`SculkSpreader.java:187-193`): the 18 offsets in
/// the 3x3x3 box that have at least one zero coordinate, excluding the origin. Built in
/// `BlockPos.betweenClosedStream` order (x fastest, then y, then z).
fn non_corner_neighbours() -> Vec<Vector3<i32>> {
    let mut list = Vec::with_capacity(18);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && !(x == 0 && y == 0 && z == 0) {
                    list.push(Vector3::new(x, y, z));
                }
            }
        }
    }
    list
}

/// `Util.shuffle` (`util/Util.java:1072-1078`): Fisher-Yates from the end.
fn shuffled_non_corner_neighbours(random: &mut RandomGenerator) -> Vec<Vector3<i32>> {
    let mut list = non_corner_neighbours();
    let size = list.len();
    for i in (2..=size).rev() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let swap_to = random.next_bounded_i32(i as i32) as usize;
        list.swap(i - 1, swap_to);
    }
    list
}

/// `ChargeCursor.isUnobstructed` (`SculkSpreader.java:355-358`).
fn is_unobstructed(
    accessor: &dyn BlockAccessor,
    from: BlockPos,
    direction: BlockDirection,
) -> bool {
    let test_pos = from.offset(direction.to_offset());
    !accessor
        .get_block_state(&test_pos)
        .is_side_solid(direction.opposite())
}

/// `ChargeCursor.isMovementUnobstructed` (`SculkSpreader.java:331-353`).
fn is_movement_unobstructed(accessor: &dyn BlockAccessor, from: BlockPos, to: BlockPos) -> bool {
    if from.manhattan_distance(to) == 1 {
        return true;
    }
    let dx = to.0.x - from.0.x;
    let dy = to.0.y - from.0.y;
    let dz = to.0.z - from.0.z;
    let direction_x = if dx < 0 {
        BlockDirection::West
    } else {
        BlockDirection::East
    };
    let direction_y = if dy < 0 {
        BlockDirection::Down
    } else {
        BlockDirection::Up
    };
    let direction_z = if dz < 0 {
        BlockDirection::North
    } else {
        BlockDirection::South
    };
    if dx == 0 {
        is_unobstructed(accessor, from, direction_y) || is_unobstructed(accessor, from, direction_z)
    } else if dy == 0 {
        is_unobstructed(accessor, from, direction_x) || is_unobstructed(accessor, from, direction_z)
    } else {
        is_unobstructed(accessor, from, direction_x) || is_unobstructed(accessor, from, direction_y)
    }
}

/// `ChargeCursor.getValidMovementPos` (`SculkSpreader.java:313-329`).
fn get_valid_movement_pos(
    accessor: &dyn BlockAccessor,
    pos: BlockPos,
    random: &mut RandomGenerator,
) -> Option<BlockPos> {
    let mut sculk_position = pos;
    for offset in shuffled_non_corner_neighbours(random) {
        let neighbour = pos.offset(offset);
        let transferee = accessor.get_block(&neighbour);
        if is_sculk_behaviour_block(transferee)
            && is_movement_unobstructed(accessor, pos, neighbour)
        {
            sculk_position = neighbour;
            if sculk_vein::has_substrate_access(accessor, neighbour) {
                break;
            }
        }
    }
    (sculk_position != pos).then_some(sculk_position)
}

/// One entry of the `levelEvent(3006, ...)` burst `updateCursors` emits after processing
/// (`SculkSpreader.java:150` and `:170-180`). `data == 0` is the "cursor died here" pop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChargeParticleEvent {
    pub pos: BlockPos,
    pub data: i32,
}

/// `SculkSpreader`.
#[derive(Debug, Clone)]
pub struct SculkSpreader {
    config: SculkSpreaderConfig,
    cursors: Vec<ChargeCursor>,
}

impl SculkSpreader {
    /// `SculkSpreader.createLevelSpreader` (`SculkSpreader.java:67-69`).
    #[must_use]
    pub const fn level_spreader() -> Self {
        Self {
            config: SculkSpreaderConfig::level_spreader(),
            cursors: Vec::new(),
        }
    }

    /// `SculkSpreader.createWorldGenSpreader` (`SculkSpreader.java:71-73`).
    #[must_use]
    pub const fn world_gen_spreader() -> Self {
        Self {
            config: SculkSpreaderConfig::world_gen_spreader(),
            cursors: Vec::new(),
        }
    }

    #[must_use]
    pub const fn config(&self) -> &SculkSpreaderConfig {
        &self.config
    }

    /// `getCursors` (`SculkSpreader.java:99-102`).
    #[must_use]
    pub fn cursors(&self) -> &[ChargeCursor] {
        &self.cursors
    }

    /// `clear` (`SculkSpreader.java:104-106`).
    pub fn clear(&mut self) {
        self.cursors.clear();
    }

    /// `addCursors` (`SculkSpreader.java:126-132`): a charge larger than `MAX_CHARGE` is
    /// split across several cursors at the same position.
    pub fn add_cursors(&mut self, start_pos: BlockPos, mut charge: i32) {
        while charge > 0 {
            let current_charge = charge.min(MAX_CHARGE);
            self.add_cursor(ChargeCursor::fresh(start_pos, current_charge));
            charge -= current_charge;
        }
    }

    /// `addCursor` (`SculkSpreader.java:134-138`): silently dropped past 32 cursors.
    pub fn add_cursor(&mut self, cursor: ChargeCursor) {
        if self.cursors.len() < MAX_CURSORS {
            self.cursors.push(cursor);
        }
    }

    /// `load` (`SculkSpreader.java:108-111`): reads the `cursors` list, size-limited to 32.
    pub fn load_nbt(&mut self, nbt: &NbtCompound) {
        self.cursors.clear();
        let Some(list) = nbt.get_list("cursors") else {
            return;
        };
        for tag in list {
            let Some(compound) = tag.extract_compound() else {
                continue;
            };
            if let Some(cursor) = cursor_from_nbt(compound) {
                self.add_cursor(cursor);
            }
        }
    }

    /// `save` (`SculkSpreader.java:113-124`), without the `DEBUG_SCULK_CATALYST` stats.
    pub fn save_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_list(
            "cursors",
            self.cursors.iter().map(cursor_to_nbt).collect::<Vec<_>>(),
        );
    }

    /// `updateCursors` (`SculkSpreader.java:140-184`).
    ///
    /// Returns the `levelEvent(3006, ...)` payloads vanilla emits inline; the caller
    /// forwards them to clients (this codebase has no `LevelAccessor.levelEvent`, only
    /// `World::sync_world_event`, which a `dyn SculkWorld` has no access to).
    pub async fn update_cursors(
        &mut self,
        world: &dyn SculkWorld,
        origin_pos: BlockPos,
        random: &mut RandomGenerator,
        spread_veins: bool,
    ) -> Vec<ChargeParticleEvent> {
        let mut events = Vec::new();
        if self.cursors.is_empty() {
            return events;
        }

        // Vanilla mutates cursors in place inside a list it then partially rebuilds; the
        // same is expressed here as "take the list, process, rebuild".
        let mut cursors = std::mem::take(&mut self.cursors);
        let mut processed: Vec<ChargeCursor> = Vec::with_capacity(cursors.len());
        // `mergeableCursors`: position -> index into `processed`.
        let mut mergeable: Vec<(BlockPos, usize)> = Vec::new();
        // `chargeMap`: position -> summed charge.
        let mut charge_map: Vec<(BlockPos, i32)> = Vec::new();

        for cursor in &mut cursors {
            if cursor.is_pos_unreasonable(origin_pos) {
                continue;
            }
            update_cursor(
                cursor,
                world,
                origin_pos,
                random,
                &self.config,
                spread_veins,
            )
            .await;
            if cursor.charge() <= 0 {
                events.push(ChargeParticleEvent {
                    pos: cursor.pos(),
                    data: 0,
                });
                continue;
            }

            let pos = cursor.pos();
            match charge_map.iter_mut().find(|(p, _)| *p == pos) {
                Some((_, total)) => *total += cursor.charge(),
                None => charge_map.push((pos, cursor.charge())),
            }

            match mergeable.iter().position(|(p, _)| *p == pos) {
                None => {
                    processed.push(*cursor);
                    mergeable.push((pos, processed.len() - 1));
                }
                Some(slot) => {
                    let existing_index = mergeable[slot].1;
                    let existing_charge = processed[existing_index].charge();
                    if !self.config.is_world_generation()
                        && cursor.charge() + existing_charge <= MAX_CHARGE
                    {
                        processed[existing_index].merge_with(cursor);
                    } else {
                        processed.push(*cursor);
                        if cursor.charge() < existing_charge {
                            mergeable[slot].1 = processed.len() - 1;
                        }
                    }
                }
            }
        }

        for (pos, charge) in charge_map {
            let Some(slot) = mergeable.iter().find(|(p, _)| *p == pos) else {
                continue;
            };
            let Some(faces) = processed[slot.1].facings() else {
                continue;
            };
            if charge > 0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
                let num_particles = (f64::from(charge).ln_1p() / 2.3f32 as f64) as i32 + 1;
                events.push(ChargeParticleEvent {
                    pos,
                    data: (num_particles << 6) + i32::from(faces.pack()),
                });
            }
        }

        self.cursors = processed;
        events
    }
}

/// `ChargeCursor.update` (`SculkSpreader.java:254-297`).
async fn update_cursor(
    cursor: &mut ChargeCursor,
    world: &dyn SculkWorld,
    origin_pos: BlockPos,
    random: &mut RandomGenerator,
    config: &SculkSpreaderConfig,
    spread_veins: bool,
) {
    // `shouldUpdate` (`SculkSpreader.java:244-252`).
    if cursor.charge() <= 0
        || (!config.is_world_generation() && !world.should_tick_blocks_at(origin_pos))
    {
        return;
    }
    if cursor.update_delay() > 0 {
        cursor.set_update_delay(cursor.update_delay() - 1);
        return;
    }

    let pos = cursor.pos();
    let mut current_state = world.accessor().get_block_state(&pos);
    let mut behaviour = behaviour_for(Block::from_state_id(current_state.id));

    if spread_veins {
        let source_faces = available_faces(current_state);
        let source_is_vein = Block::from_state_id(current_state.id) == &Block::SCULK_VEIN;
        if behaviour
            .attempt_spread_vein(world, pos, source_faces, source_is_vein, cursor.facings())
            .await
        {
            if behaviour.can_change_block_state_on_spread() {
                current_state = world.accessor().get_block_state(&pos);
                behaviour = behaviour_for(Block::from_state_id(current_state.id));
            }
            world.play_block_sound(pos, Sound::BlockSculkSpread);
        }
    }

    let new_charge = behaviour
        .attempt_use_charge(cursor, world, origin_pos, random, config, spread_veins)
        .await;
    cursor.set_charge(new_charge);

    if new_charge <= 0 {
        behaviour.on_discharged(world, cursor.pos(), random).await;
        return;
    }

    if let Some(transfer_pos) = get_valid_movement_pos(world.accessor(), cursor.pos(), random) {
        behaviour.on_discharged(world, cursor.pos(), random).await;
        cursor.set_pos(transfer_pos);
        if config.is_world_generation() {
            // `!this.pos.closerThan(new Vec3i(originX, this.pos.getY(), originZ), 15.0)`.
            let dx = f64::from(transfer_pos.0.x - origin_pos.0.x);
            let dz = f64::from(transfer_pos.0.z - origin_pos.0.z);
            if dx.mul_add(dx, dz * dz) >= 15.0 * 15.0 {
                cursor.set_charge(0);
                return;
            }
        }
    }

    let state_here = world.accessor().get_block_state(&cursor.pos());
    if is_sculk_behaviour_block(Block::from_state_id(state_here.id)) {
        cursor.set_facings(Some(available_faces(state_here)));
    }

    cursor.set_decay_delay(behaviour.update_decay_delay(cursor.decay_delay()));
    cursor.set_update_delay(i32::from(behaviour.sculk_spread_delay()));
}

/// `ChargeCursor.CODEC` (`SculkSpreader.java:201-210`), written with vanilla's field names
/// so a Pumpkin-written catalyst still loads in vanilla and vice versa.
fn cursor_to_nbt(cursor: &ChargeCursor) -> NbtTag {
    let mut compound = NbtCompound::new();
    let pos = cursor.pos();
    compound.put("pos", NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]));
    compound.put_int("charge", cursor.charge());
    compound.put_int("decay_delay", cursor.decay_delay());
    compound.put_int("update_delay", cursor.update_delay());
    if let Some(faces) = cursor.facings() {
        compound.put_list(
            "facings",
            faces
                .iter()
                .map(|d| NbtTag::String(direction_name(d).into()))
                .collect::<Vec<_>>(),
        );
    }
    NbtTag::Compound(compound)
}

fn cursor_from_nbt(compound: &NbtCompound) -> Option<ChargeCursor> {
    let pos = compound.get_int_array("pos")?;
    if pos.len() != 3 {
        return None;
    }
    let pos = BlockPos::new(pos[0], pos[1], pos[2]);
    let charge = compound.get_int("charge").unwrap_or(0).clamp(0, MAX_CHARGE);
    let decay_delay = compound.get_int("decay_delay").unwrap_or(1).clamp(0, 1);
    let update_delay = compound.get_int("update_delay").unwrap_or(0).max(0);
    let facings = compound.get_list("facings").map(|list| {
        FaceSet::from_directions(
            list.iter()
                .filter_map(|tag| tag.extract_string().and_then(direction_from_name)),
        )
    });
    Some(ChargeCursor::from_parts(
        pos,
        charge,
        decay_delay,
        update_delay,
        facings,
    ))
}

const fn direction_name(direction: BlockDirection) -> &'static str {
    match direction {
        BlockDirection::Down => "down",
        BlockDirection::Up => "up",
        BlockDirection::North => "north",
        BlockDirection::South => "south",
        BlockDirection::West => "west",
        BlockDirection::East => "east",
    }
}

fn direction_from_name(name: &str) -> Option<BlockDirection> {
    match name {
        "down" => Some(BlockDirection::Down),
        "up" => Some(BlockDirection::Up),
        "north" => Some(BlockDirection::North),
        "south" => Some(BlockDirection::South),
        "west" => Some(BlockDirection::West),
        "east" => Some(BlockDirection::East),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockFuture;
    use crate::block::blocks::multiface_spreader::{SpreadPos, SpreadTarget};
    use pumpkin_data::BlockStateId;
    use pumpkin_data::fluid::Fluid;
    use pumpkin_util::random::xoroshiro128::Xoroshiro;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeWorld {
        states: Mutex<HashMap<BlockPos, &'static BlockState>>,
        default: &'static BlockState,
    }

    impl FakeWorld {
        fn new(default: &'static BlockState) -> Self {
            Self {
                states: Mutex::new(HashMap::new()),
                default,
            }
        }

        fn set(&self, pos: BlockPos, state: &'static BlockState) {
            self.states.lock().unwrap().insert(pos, state);
        }
    }

    impl BlockAccessor for FakeWorld {
        fn get_block(&self, position: &BlockPos) -> &'static Block {
            Block::from_state_id(self.get_block_state(position).id)
        }
        fn get_block_state(&self, position: &BlockPos) -> &'static BlockState {
            self.states
                .lock()
                .unwrap()
                .get(position)
                .copied()
                .unwrap_or(self.default)
        }
        fn get_block_state_id(&self, position: &BlockPos) -> BlockStateId {
            self.get_block_state(position).id
        }
        fn get_block_and_state(
            &self,
            position: &BlockPos,
        ) -> (&'static Block, &'static BlockState) {
            let state = self.get_block_state(position);
            (Block::from_state_id(state.id), state)
        }
        fn get_fluid(&self, _position: &BlockPos) -> Fluid {
            Fluid::EMPTY
        }
    }

    impl SpreadTarget for FakeWorld {
        fn accessor(&self) -> &dyn BlockAccessor {
            self
        }
        fn place(&self, _spread_pos: SpreadPos) -> BlockFuture<'_, bool> {
            Box::pin(async { false })
        }
    }

    impl SculkWorld for FakeWorld {
        fn set_block(&self, pos: BlockPos, state_id: BlockStateId) -> BlockFuture<'_, ()> {
            Box::pin(async move {
                self.states.lock().unwrap().insert(pos, state_id.to_state());
            })
        }
        fn play_block_sound(&self, _pos: BlockPos, _sound: Sound) {}
        fn push_entities_up(&self, _pos: BlockPos) {}
    }

    fn random() -> RandomGenerator {
        RandomGenerator::Xoroshiro(Xoroshiro::from_seed(42))
    }

    #[test]
    fn add_cursors_splits_charge_at_max_charge() {
        let mut spreader = SculkSpreader::level_spreader();
        spreader.add_cursors(BlockPos::new(0, 0, 0), 2500);
        let charges: Vec<i32> = spreader
            .cursors()
            .iter()
            .map(ChargeCursor::charge)
            .collect();
        assert_eq!(charges, vec![1000, 1000, 500]);
    }

    #[test]
    fn add_cursors_is_capped_at_thirty_two() {
        let mut spreader = SculkSpreader::level_spreader();
        spreader.add_cursors(BlockPos::new(0, 0, 0), 40 * MAX_CHARGE);
        assert_eq!(spreader.cursors().len(), MAX_CURSORS);
    }

    #[test]
    fn fresh_cursor_matches_vanilla_defaults() {
        let cursor = ChargeCursor::fresh(BlockPos::new(1, 2, 3), 7);
        assert_eq!(cursor.decay_delay(), 1);
        assert_eq!(cursor.update_delay(), 0);
        assert_eq!(cursor.facings(), None);
    }

    #[test]
    fn behaviour_lookup_covers_the_two_vanilla_implementors() {
        assert!(!behaviour_for(&Block::SCULK).can_change_block_state_on_spread());
        assert!(behaviour_for(&Block::SCULK_VEIN).can_change_block_state_on_spread());
        // `SculkBehaviour.DEFAULT` for everything else: its `updateDecayDelay` decrements.
        assert_eq!(behaviour_for(&Block::STONE).update_decay_delay(1), 0);
    }

    #[test]
    fn non_corner_neighbours_has_the_vanilla_eighteen() {
        let list = non_corner_neighbours();
        assert_eq!(list.len(), 18);
        assert!(!list.iter().any(|v| v.x == 0 && v.y == 0 && v.z == 0));
        assert!(
            !list.iter().any(|v| v.x != 0 && v.y != 0 && v.z != 0),
            "corner offsets must be excluded"
        );
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut rng = random();
        let mut shuffled = shuffled_non_corner_neighbours(&mut rng);
        let mut original = non_corner_neighbours();
        shuffled.sort_by_key(|v| (v.x, v.y, v.z));
        original.sort_by_key(|v| (v.x, v.y, v.z));
        assert_eq!(shuffled, original);
    }

    #[test]
    fn movement_is_unobstructed_for_direct_neighbours() {
        let world = FakeWorld::new(Block::STONE.default_state);
        let from = BlockPos::new(0, 0, 0);
        assert!(is_movement_unobstructed(
            &world,
            from,
            BlockPos::new(1, 0, 0)
        ));
    }

    #[test]
    fn diagonal_movement_is_blocked_by_solid_neighbours() {
        let world = FakeWorld::new(Block::STONE.default_state);
        let from = BlockPos::new(0, 0, 0);
        // Both orthogonal detours are solid stone, so the diagonal is obstructed.
        assert!(!is_movement_unobstructed(
            &world,
            from,
            BlockPos::new(1, 1, 0)
        ));
    }

    #[test]
    fn diagonal_movement_is_allowed_through_air() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let from = BlockPos::new(0, 0, 0);
        assert!(is_movement_unobstructed(
            &world,
            from,
            BlockPos::new(1, 1, 0)
        ));
    }

    #[test]
    fn valid_movement_pos_only_targets_sculk_behaviour_blocks() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let origin = BlockPos::new(0, 0, 0);
        let mut rng = random();
        assert_eq!(get_valid_movement_pos(&world, origin, &mut rng), None);

        world.set(BlockPos::new(1, 0, 0), Block::SCULK.default_state);
        let mut rng = random();
        assert_eq!(
            get_valid_movement_pos(&world, origin, &mut rng),
            Some(BlockPos::new(1, 0, 0))
        );
    }

    #[test]
    fn cursor_nbt_round_trips_with_vanilla_field_names() {
        let mut spreader = SculkSpreader::level_spreader();
        let mut cursor = ChargeCursor::fresh(BlockPos::new(-3, 70, 12), 250);
        cursor.set_update_delay(4);
        cursor.set_facings(Some(FaceSet::from_directions([
            BlockDirection::Up,
            BlockDirection::North,
        ])));
        spreader.add_cursor(cursor);

        let mut nbt = NbtCompound::new();
        spreader.save_nbt(&mut nbt);
        assert!(nbt.get_list("cursors").is_some());

        let mut loaded = SculkSpreader::level_spreader();
        loaded.load_nbt(&nbt);
        assert_eq!(loaded.cursors(), spreader.cursors());
    }

    #[test]
    fn load_replaces_any_previous_cursors_and_respects_the_cap() {
        let mut source = SculkSpreader::level_spreader();
        source.add_cursors(BlockPos::new(0, 0, 0), 3 * MAX_CHARGE);
        let mut nbt = NbtCompound::new();
        source.save_nbt(&mut nbt);

        let mut target = SculkSpreader::level_spreader();
        target.add_cursors(BlockPos::new(9, 9, 9), MAX_CHARGE);
        target.load_nbt(&nbt);
        assert_eq!(target.cursors().len(), 3);
        assert!(
            target
                .cursors()
                .iter()
                .all(|c| c.pos() == BlockPos::new(0, 0, 0))
        );
    }

    #[tokio::test]
    async fn cursors_at_the_same_position_merge_below_max_charge() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let pos = BlockPos::new(0, 0, 0);
        world.set(pos, Block::SCULK.default_state);
        let mut spreader = SculkSpreader::level_spreader();
        // Two low-charge cursors that cannot move (no sculk neighbours) and sum under 1000.
        spreader.add_cursor(ChargeCursor::fresh(pos, 100));
        spreader.add_cursor(ChargeCursor::fresh(pos, 100));

        let mut rng = random();
        spreader.update_cursors(&world, pos, &mut rng, false).await;
        assert_eq!(spreader.cursors().len(), 1);
        assert!(spreader.cursors()[0].charge() >= 190);
    }

    #[tokio::test]
    async fn unreasonably_distant_cursors_are_dropped() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let mut spreader = SculkSpreader::level_spreader();
        spreader.add_cursor(ChargeCursor::fresh(BlockPos::new(5000, 0, 0), 500));
        let mut rng = random();
        let events = spreader
            .update_cursors(&world, BlockPos::new(0, 0, 0), &mut rng, false)
            .await;
        assert!(spreader.cursors().is_empty());
        assert!(events.is_empty(), "dropped cursors emit no particle event");
    }

    #[tokio::test]
    async fn a_discharged_cursor_emits_a_pop_event_and_leaves_the_list() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let pos = BlockPos::new(0, 0, 0);
        let mut spreader = SculkSpreader::level_spreader();
        // Charge 0 on air: DEFAULT holds it at 0, so the cursor is culled.
        spreader.add_cursor(ChargeCursor::from_parts(pos, 1, 0, 0, None));
        let mut rng = random();
        let events = spreader.update_cursors(&world, pos, &mut rng, false).await;
        assert!(spreader.cursors().is_empty());
        assert_eq!(events, vec![ChargeParticleEvent { pos, data: 0 }]);
    }

    #[tokio::test]
    async fn a_cursor_on_sculk_moves_onto_an_adjacent_sculk_block() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let start = BlockPos::new(0, 0, 0);
        let neighbour = BlockPos::new(1, 0, 0);
        world.set(start, Block::SCULK.default_state);
        world.set(neighbour, Block::SCULK.default_state);

        let mut spreader = SculkSpreader::level_spreader();
        spreader.add_cursor(ChargeCursor::fresh(start, MAX_CHARGE));
        let mut rng = random();
        spreader
            .update_cursors(&world, start, &mut rng, false)
            .await;
        assert_eq!(spreader.cursors().len(), 1);
        assert_eq!(spreader.cursors()[0].pos(), neighbour);
        // Sitting on a SculkBehaviour block refreshes `facings` from an empty set.
        assert_eq!(spreader.cursors()[0].facings(), Some(FaceSet::EMPTY));
    }

    #[tokio::test]
    async fn update_delay_is_consumed_before_any_charge_work() {
        let world = FakeWorld::new(Block::AIR.default_state);
        let pos = BlockPos::new(0, 0, 0);
        world.set(pos, Block::SCULK.default_state);
        let mut spreader = SculkSpreader::level_spreader();
        spreader.add_cursor(ChargeCursor::from_parts(pos, 500, 1, 3, None));
        let mut rng = random();
        spreader.update_cursors(&world, pos, &mut rng, false).await;
        assert_eq!(spreader.cursors()[0].update_delay(), 2);
        assert_eq!(spreader.cursors()[0].charge(), 500);
        assert_eq!(spreader.cursors()[0].pos(), pos);
    }
}
