use crate::entity::ai::goal::move_to_target_pos::MoveToTargetPos;
use crate::entity::ai::goal::step_and_destroy_block::{
    StepAndDestroyBlockGoal, Stepping, SteppingFuture,
};
use crate::entity::ai::goal::{Controls, Goal, GoalFuture, ParentHandle};
use crate::entity::mob::Mob;
use crate::world::World;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockStateId, particle::Particle};
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackTemplateSerializer;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::{RngExt, rng};
use std::pin::Pin;
use std::sync::Arc;

pub struct DestroyEggGoal {
    step_and_destroy_block_goal: StepAndDestroyBlockGoal<Self, Self>,
}

impl DestroyEggGoal {
    fn spawn_nearby_particle(
        world: &World,
        position: Vector3<f64>,
        offset: Vector3<f32>,
        max_speed: f32,
        particle_count: i32,
        particle: Particle,
    ) {
        let players = world.players.load();
        for player in players.iter() {
            let player_block = player.living_entity.entity.block_pos.load();
            let dx = f64::from(player_block.0.x) + 0.5 - position.x;
            let dy = f64::from(player_block.0.y) + 0.5 - position.y;
            let dz = f64::from(player_block.0.z) + 0.5 - position.z;
            if dx.mul_add(dx, dy.mul_add(dy, dz * dz)) <= 32.0 * 32.0 {
                player.spawn_particle(position, offset, max_speed, particle_count, particle);
            }
        }
    }

    fn spawn_nearby_item_particle(world: &World, position: Vector3<f64>, offset: Vector3<f32>) {
        let item = ItemStackTemplateSerializer::from(ItemStack::new(1, &Item::EGG));
        let players = world.players.load();
        for player in players.iter() {
            let player_block = player.living_entity.entity.block_pos.load();
            let dx = f64::from(player_block.0.x) + 0.5 - position.x;
            let dy = f64::from(player_block.0.y) + 0.5 - position.y;
            let dz = f64::from(player_block.0.z) + 0.5 - position.z;
            if dx.mul_add(dx, dy.mul_add(dy, dz * dz)) > 32.0 * 32.0 {
                continue;
            }

            let mut data = Vec::new();
            if item
                .write_with_version(&mut data, &player.client.java_version())
                .is_ok()
            {
                player.spawn_particle_with_data(position, offset, 0.15, 3, Particle::Item, &data);
            }
        }
    }

    #[must_use]
    pub fn new(speed: f64, max_y_difference: i32) -> Box<Self> {
        let mut this = Box::new(Self {
            step_and_destroy_block_goal: StepAndDestroyBlockGoal::new(
                ParentHandle::none(),
                ParentHandle::none(),
                &Block::TURTLE_EGG,
                speed,
                max_y_difference,
            ),
        });

        // SAFETY: `this` heap allocation address is pinned in Box and outlives `ParentHandle` references.
        this.step_and_destroy_block_goal.stepping = unsafe { ParentHandle::new(&this) };
        this.step_and_destroy_block_goal.move_to_target_pos_goal.move_to_target_pos =
            // SAFETY: `this` heap allocation address is pinned in Box and outlives `ParentHandle` references.
            unsafe { ParentHandle::new(&this) };

        this
    }
}

impl Goal for DestroyEggGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.step_and_destroy_block_goal.can_start(mob).await })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { self.step_and_destroy_block_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.step_and_destroy_block_goal.start(mob).await;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.step_and_destroy_block_goal.stop(mob).await;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            self.step_and_destroy_block_goal.tick(mob).await;
        })
    }

    fn should_run_every_tick(&self) -> bool {
        self.step_and_destroy_block_goal.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.step_and_destroy_block_goal.controls()
    }
}

impl Stepping for DestroyEggGoal {
    fn tick_stepping(&self, world: Arc<World>, block_pos: BlockPos) -> SteppingFuture<'_> {
        Box::pin(async move {
            let random = rng().random::<f32>();

            // NOTE: block_pos.0.to_f64() is assumed to be the correct way to get Vector3<f64>
            let pos_f64 = (block_pos.0).to_f64();

            world.play_sound_raw(
                Sound::EntityZombieDestroyEgg as u16,
                SoundCategory::Hostile,
                &pos_f64,
                0.7,
                random.mul_add(0.2, 0.9),
            );
        })
    }

    fn on_destroy_block(&self, world: Arc<World>, block_pos: BlockPos) -> SteppingFuture<'_> {
        Box::pin(async move {
            let random = rng().random::<f32>();

            let expected_state = world.get_block_state(&block_pos).id;
            if Block::from_state_id(expected_state).id != Block::TURTLE_EGG.id
                || !world
                    .set_block_state_if(
                        &block_pos,
                        expected_state,
                        BlockStateId::AIR,
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await
            {
                return;
            }

            // Vanilla RemoveBlockGoal calls Level.removeBlock(pos, false), which
            // replaces the block with its fluid state without drops or a player
            // break event.
            let position = Vector3::new(
                f64::from(block_pos.0.x) + 0.5,
                f64::from(block_pos.0.y),
                f64::from(block_pos.0.z) + 0.5,
            );
            let mut particle_random = rng();
            for _ in 0..20 {
                let mut next_gaussian = || {
                    let u1 = particle_random.random::<f64>().max(f64::MIN_POSITIVE);
                    let u2 = particle_random.random::<f64>();
                    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
                };
                Self::spawn_nearby_particle(
                    &world,
                    position,
                    Vector3::new(
                        (next_gaussian() * 0.02) as f32,
                        (next_gaussian() * 0.02) as f32,
                        (next_gaussian() * 0.02) as f32,
                    ),
                    0.15,
                    1,
                    Particle::Poof,
                );
            }

            // NOTE: block_pos.0.to_f64() is assumed to be the correct way to get Vector3<f64>
            let pos_f64 = (block_pos.0).to_f64();

            world.play_sound_raw(
                Sound::EntityTurtleEggBreak as u16,
                SoundCategory::Blocks,
                &pos_f64,
                0.7,
                random.mul_add(0.2, 0.9),
            );
        })
    }

    fn tick_stepping_particles(
        &self,
        world: Arc<World>,
        block_pos: BlockPos,
    ) -> SteppingFuture<'_> {
        Box::pin(async move {
            let mut random = rng();
            Self::spawn_nearby_item_particle(
                &world,
                Vector3::new(
                    f64::from(block_pos.0.x) + 0.5,
                    f64::from(block_pos.0.y) + 0.7,
                    f64::from(block_pos.0.z) + 0.5,
                ),
                Vector3::new(
                    (random.random::<f32>() - 0.5) * 0.08,
                    (random.random::<f32>() - 0.5) * 0.08,
                    (random.random::<f32>() - 0.5) * 0.08,
                ),
            );
        })
    }
}

impl MoveToTargetPos for DestroyEggGoal {
    fn is_target_pos<'a>(
        &'a self,
        world: Arc<World>,
        block_pos: BlockPos,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            self.step_and_destroy_block_goal
                .is_target_pos(world, block_pos)
                .await
        })
    }

    fn get_desired_distance_to_target(&self) -> f64 {
        1.14
    }
}
