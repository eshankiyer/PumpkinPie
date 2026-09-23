use pumpkin_data::BlockStateId;
use pumpkin_data::particle::Particle;
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::block::blocks::plant::PlantBlockBase;
use crate::block::{BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetStateForNeighborUpdateArgs};

/// `EyeblossomBlock.Type.OPEN` particle color.
pub const EYEBLOSSOM_OPEN_COLOR: i32 = 16_545_810;
/// `EyeblossomBlock.Type.CLOSED` particle color.
pub const EYEBLOSSOM_CLOSED_COLOR: i32 = 6_250_335;

/// Vanilla `EyeblossomBlock.Type.spawnTransformParticle`: a trail particle drifting from
/// the block's center to a random nearby point, over a random 0.5-1.5s lifetime.
///
/// The eyeblossom open/close logic itself lives in `plant::eyeblossom::EyeblossomBlock`;
/// this helper encodes the `TrailParticleOption` payload (target, color, duration) that a
/// bare `Particle::Trail` spawn lacks.
pub fn spawn_transform_particle(
    world: &crate::world::World,
    position: &pumpkin_util::math::position::BlockPos,
    color: i32,
) {
    let start = Vector3::new(
        f64::from(position.0.x) + 0.5,
        f64::from(position.0.y) + 0.5,
        f64::from(position.0.z) + 0.5,
    );
    let mut rng = rand::rng();
    let lifetime = 0.5 + rng.random::<f64>();
    let velocity = Vector3::new(
        rng.random::<f64>() - 0.5,
        rng.random::<f64>() + 1.0,
        rng.random::<f64>() - 0.5,
    );
    let target = start + velocity * lifetime;
    let duration = (20.0 * lifetime) as i32;

    let mut data = Vec::new();
    let _ = data.write_f64_be(target.x);
    let _ = data.write_f64_be(target.y);
    let _ = data.write_f64_be(target.z);
    let _ = data.write_i32_be(color);
    let _ = VarInt(duration).encode(&mut data);

    world.spawn_particle_with_data(
        start,
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
        1,
        Particle::Trail,
        &data,
    );
}

#[pumpkin_block_from_tag("minecraft:small_flowers")]
pub struct FlowerBlock;

impl BlockBehaviour for FlowerBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        <Self as PlantBlockBase>::can_place_at(self, args.block_accessor, args.position)
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            <Self as PlantBlockBase>::get_state_for_neighbor_update(
                self,
                args.world,
                args.position,
                args.state_id,
            )
            .await
        })
    }
}

impl PlantBlockBase for FlowerBlock {}
