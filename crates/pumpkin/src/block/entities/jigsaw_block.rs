use rand::Rng;
use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};

use futures::Future;
use pumpkin_data::block_properties::{BlockProperties, JigsawLikeProperties, Orientation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, xoroshiro128::Xoroshiro},
};
use pumpkin_world::generation::structure::structures::{
    StructureGeneratorContext, StructurePosition,
    jigsaw::{JigsawJointType, PoolElementStructurePiece},
    jigsaw_placement::{
        DimensionPadding, JigsawPlacement, LiquidSettings, MaxDistance, PoolAliasLookup,
    },
};

use tokio::sync::Mutex;

use crate::block::blocks::jigsaw::JigsawBlock;
use crate::world::World;

use super::BlockEntity;

pub struct JigsawBlockEntity {
    pub position: BlockPos,
    pub name: Mutex<String>,
    pub target: Mutex<String>,
    pub pool: Mutex<String>,
    pub final_state: Mutex<String>,
    pub joint: Mutex<JigsawJointType>,
    pub selection_priority: AtomicI32,
    pub placement_priority: AtomicI32,
    pub dirty: AtomicBool,
}

impl JigsawBlockEntity {
    pub const ID: &'static str = "minecraft:jigsaw";
    pub const EMPTY_ID: &'static str = "minecraft:empty";
    pub const DEFAULT_FINAL_STATE: &'static str = "minecraft:air";
    pub const DEFAULT_PLACEMENT_PRIORITY: i32 = 0;
    pub const DEFAULT_SELECTION_PRIORITY: i32 = 0;
    pub const NAME: &'static str = "name";
    pub const TARGET: &'static str = "target";
    pub const POOL: &'static str = "pool";
    pub const FINAL_STATE: &'static str = "final_state";
    pub const JOINT: &'static str = "joint";
    pub const PLACEMENT_PRIORITY: &'static str = "placement_priority";
    pub const SELECTION_PRIORITY: &'static str = "selection_priority";

    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            name: Mutex::new(Self::EMPTY_ID.to_string()),
            target: Mutex::new(Self::EMPTY_ID.to_string()),
            pool: Mutex::new(Self::EMPTY_ID.to_string()),
            final_state: Mutex::new(Self::DEFAULT_FINAL_STATE.to_string()),
            joint: Mutex::new(JigsawJointType::Rollable),
            selection_priority: AtomicI32::new(Self::DEFAULT_SELECTION_PRIORITY),
            placement_priority: AtomicI32::new(Self::DEFAULT_PLACEMENT_PRIORITY),
            dirty: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub const fn get_default_joint_type(orientation: Orientation) -> JigsawJointType {
        let front = JigsawBlock::get_front_facing(orientation);
        if front.is_horizontal() {
            JigsawJointType::Aligned
        } else {
            JigsawJointType::Rollable
        }
    }

    // Vanilla exposes this state through `getJoint` and the six setters below
    // (`JigsawBlockEntity.java:66-104`). These methods are used by the live
    // jigsaw packet handler and the block-entity serialization path.
    pub async fn get_joint(&self) -> JigsawJointType {
        *self.joint.lock().await
    }

    pub async fn set_name(&self, name: String) {
        *self.name.lock().await = name;
    }

    pub async fn set_pool(&self, pool: String) {
        *self.pool.lock().await = pool;
    }

    pub async fn set_final_state(&self, final_state: String) {
        *self.final_state.lock().await = final_state;
    }

    pub async fn set_joint(&self, joint: JigsawJointType) {
        *self.joint.lock().await = joint;
    }

    pub fn set_placement_priority(&self, placement_priority: i32) {
        self.placement_priority
            .store(placement_priority, Ordering::SeqCst);
    }

    pub fn set_selection_priority(&self, selection_priority: i32) {
        self.selection_priority
            .store(selection_priority, Ordering::SeqCst);
    }

    pub async fn generate(&self, world: &Arc<World>, levels: i32, keep_jigsaws: bool) {
        let pool = self.pool.lock().await.clone();
        let target = self.target.lock().await.clone();

        let block_state = world.get_block_state(&self.position);
        let props =
            JigsawLikeProperties::from_state_id(block_state.id, &pumpkin_data::Block::JIGSAW);
        let front = JigsawBlock::get_front_facing(props.r#orientation);

        let position = self.position.offset(front.to_offset());

        let structure = {
            let mut context = StructureGeneratorContext {
                seed: world.level_info.load().world_gen_settings.seed,
                chunk_x: position.chunk_position().x,
                chunk_z: position.chunk_position().y,
                random: RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().next_u64())),
                sea_level: 63,
                min_y: -64,
                height_sampler: None,
                structure_key: None,
            };

            JigsawPlacement::add_pieces(
                &mut context,
                &pool,
                Some(&target),
                levels,
                position,
                false,
                false,
                &MaxDistance::new(128),
                DimensionPadding::ZERO,
                LiquidSettings::ApplyWaterlog,
                &PoolAliasLookup::default(),
            )
        };

        if let Some(structure) = structure {
            self.place_structure(world, structure, keep_jigsaws).await;
        }
    }

    async fn place_structure(
        &self,
        world: &Arc<World>,
        structure: StructurePosition,
        keep_jigsaws: bool,
    ) {
        let mut pieces = std::mem::take(
            &mut structure
                .collector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pieces,
        );
        let mut placer = crate::world::block_placer::WorldBlockPlacer::new(world);
        for piece in &mut pieces {
            if let Some(pool_piece) = piece.as_any().downcast_ref::<PoolElementStructurePiece>() {
                pumpkin_world::generation::structure::structures::jigsaw::place_pool_element_templates(
                    pool_piece,
                    &mut placer,
                    None,
                    keep_jigsaws,
                );
            }
        }
        placer.finalize().await;
        world.queue_block_updates(&placer.changed_positions).await;
        world.flush_block_updates().await;
    }
}

impl BlockEntity for JigsawBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }
    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let name = Mutex::new(
            nbt.get_string(Self::NAME)
                .unwrap_or(Self::EMPTY_ID)
                .to_string(),
        );
        let target = Mutex::new(
            nbt.get_string(Self::TARGET)
                .unwrap_or(Self::EMPTY_ID)
                .to_string(),
        );
        let pool = Mutex::new(
            nbt.get_string(Self::POOL)
                .unwrap_or(Self::EMPTY_ID)
                .to_string(),
        );
        let final_state = Mutex::new(
            nbt.get_string(Self::FINAL_STATE)
                .unwrap_or(Self::DEFAULT_FINAL_STATE)
                .to_string(),
        );
        let joint = Mutex::new(
            nbt.get_string(Self::JOINT)
                .map_or(JigsawJointType::Rollable, JigsawJointType::from_str),
        );
        let selection_priority = AtomicI32::new(
            nbt.get_int(Self::SELECTION_PRIORITY)
                .unwrap_or(Self::DEFAULT_SELECTION_PRIORITY),
        );
        let placement_priority = AtomicI32::new(
            nbt.get_int(Self::PLACEMENT_PRIORITY)
                .unwrap_or(Self::DEFAULT_PLACEMENT_PRIORITY),
        );

        Self {
            position,
            name,
            target,
            pool,
            final_state,
            joint,
            selection_priority,
            placement_priority,
            dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            nbt.put_string(Self::NAME, self.name.lock().await.clone());
            nbt.put_string(Self::TARGET, self.target.lock().await.clone());
            nbt.put_string(Self::POOL, self.pool.lock().await.clone());
            nbt.put_string(Self::FINAL_STATE, self.final_state.lock().await.clone());
            // The vanilla save path persists the joint selected by getJoint
            // (`JigsawBlockEntity.java:107-116`).
            let joint = self.get_joint().await;
            nbt.put_string(Self::JOINT, joint.as_str().to_string());
            nbt.put_int(
                Self::PLACEMENT_PRIORITY,
                self.placement_priority.load(Ordering::SeqCst),
            );
            nbt.put_int(
                Self::SELECTION_PRIORITY,
                self.selection_priority.load(Ordering::SeqCst),
            );
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_string(Self::NAME, self.name.try_lock().ok()?.clone());
        nbt.put_string(Self::TARGET, self.target.try_lock().ok()?.clone());
        nbt.put_string(Self::POOL, self.pool.try_lock().ok()?.clone());
        nbt.put_string(Self::FINAL_STATE, self.final_state.try_lock().ok()?.clone());
        let joint = *self.joint.try_lock().ok()?;
        nbt.put_string(Self::JOINT, joint.as_str().to_string());
        nbt.put_int(
            Self::PLACEMENT_PRIORITY,
            self.placement_priority.load(Ordering::SeqCst),
        );
        nbt.put_int(
            Self::SELECTION_PRIORITY,
            self.selection_priority.load(Ordering::SeqCst),
        );
        Some(nbt)
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{JigsawBlockEntity, JigsawJointType};
    use pumpkin_util::math::position::BlockPos;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn setters_update_jigsaw_configuration() {
        // These fields correspond to the vanilla JigsawBlockEntity accessors
        // (`JigsawBlockEntity.java:66-104`).
        let entity = JigsawBlockEntity::new(BlockPos::new(0, 64, 0));

        entity.set_name("minecraft:entrance".to_owned()).await;
        entity
            .set_pool("minecraft:village/plains/houses".to_owned())
            .await;
        entity.set_final_state("minecraft:stone".to_owned()).await;
        entity.set_joint(JigsawJointType::Aligned).await;
        entity.set_placement_priority(3);
        entity.set_selection_priority(7);

        assert_eq!(&*entity.name.lock().await, "minecraft:entrance");
        assert_eq!(
            &*entity.pool.lock().await,
            "minecraft:village/plains/houses"
        );
        assert_eq!(&*entity.final_state.lock().await, "minecraft:stone");
        assert_eq!(entity.get_joint().await, JigsawJointType::Aligned);
        assert_eq!(entity.placement_priority.load(Ordering::SeqCst), 3);
        assert_eq!(entity.selection_priority.load(Ordering::SeqCst), 7);
    }
}
