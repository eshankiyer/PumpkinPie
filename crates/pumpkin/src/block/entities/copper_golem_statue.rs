use std::pin::Pin;
use std::sync::Arc;

use pumpkin_data::Block;
use pumpkin_data::block_properties::HorizontalFacing;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{CustomNameImpl, DataComponentImpl};
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;

use crate::entity::Entity;
use crate::entity::passive::copper_golem::CopperGolemEntity;
use crate::world::World;

use super::BlockEntity;

pub struct CopperGolemStatueBlockEntity {
    pub position: BlockPos,
}

impl BlockEntity for CopperGolemStatueBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(_nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        Self { position }
    }

    fn write_nbt<'a>(
        &'a self,
        _nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        Some(NbtCompound::new())
    }
}

impl CopperGolemStatueBlockEntity {
    pub const ID: &'static str = "minecraft:copper_golem_statue";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self { position }
    }

    /// `CopperGolemStatueBlockEntity.createStatue` (`CopperGolemStatueBlockEntity.java:22-25`)
    /// additionally stores the golem's custom name as the statue's `CUSTOM_NAME` component.
    /// The caller (`CopperGolemEntity` turning into a statue) still calls this without the
    /// golem's name; [`Self::store_custom_name`] is the component write it will need.
    pub const fn create_statue(&self) {}

    /// Stores `custom_name` as the statue's `CUSTOM_NAME` component next to whatever other
    /// components it holds; `None` clears it (`CopperGolemStatueBlockEntity.java:22-25`).
    pub fn store_custom_name(&self, world: &World, custom_name: Option<TextComponent>) {
        let mut components = world
            .get_block_entity_components(&self.position)
            .unwrap_or_default();
        components
            .child_tags
            .remove(DataComponent::CustomName.to_name());
        if let Some(name) = custom_name {
            components.put(
                DataComponent::CustomName.to_name(),
                CustomNameImpl { name }.write_data(),
            );
        }
        world.set_block_entity_components(&self.position, components);
    }

    /// `CopperGolemStatueBlockEntity.removeStatue` + `initCopperGolem`: spawns a fresh
    /// (`UNAFFECTED`) copper golem at this position, facing the direction the statue block
    /// was facing, and removes the statue block.
    ///
    /// Callers must first check `WeatheringCopperGolemStatueBlock.useItemOn`'s condition
    /// (an axe used on a statue whose weathering stage is `UNAFFECTED`, i.e. the unwaxed
    /// `minecraft:copper_golem_statue` alone) -- this method performs the conversion
    /// unconditionally once called. `CopperGolemStatueBlock::use_with_item` is that caller.
    ///
    /// `waterlogged` mirrors `Level.removeBlock(pos, false)`, which replaces the block with
    /// `getFluidState(pos).createLegacyBlock()`: a waterlogged statue leaves a water source
    /// behind, not air.
    pub async fn remove_statue(
        &self,
        world: &Arc<World>,
        facing: HorizontalFacing,
        waterlogged: bool,
    ) {
        let pos = self.position;
        let center = Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y),
            f64::from(pos.0.z) + 0.5,
        );

        let entity = Entity::new(world.clone(), center, &EntityType::COPPER_GOLEM);
        // `copperGolem.setCustomName(this.components().get(CUSTOM_NAME))`
        // (`CopperGolemStatueBlockEntity.java:30-33`).
        let custom_name = world
            .get_block_entity_components(&pos)
            .and_then(|components| {
                components
                    .get(DataComponent::CustomName.to_name())
                    .and_then(CustomNameImpl::read_data)
            })
            .map(|component| component.name);
        if let Some(name) = custom_name {
            entity.custom_name.store(Arc::new(Some(name)));
        }
        let yaw = horizontal_facing_to_yaw(facing);
        entity.yaw.store(yaw);
        entity.head_yaw.store(yaw);

        let golem = CopperGolemEntity::new(entity);
        world.spawn_entity(golem).await;

        let remaining = if waterlogged {
            Block::WATER.default_state.id
        } else {
            Block::AIR.default_state.id
        };
        world
            .set_block_state(&pos, remaining, BlockFlags::NOTIFY_ALL)
            .await;
    }
}

/// Inverse of `Entity::get_horizontal_facing`'s `floor(yaw / 90 + 0.5) & 3` mapping.
const fn horizontal_facing_to_yaw(facing: HorizontalFacing) -> f32 {
    match facing {
        HorizontalFacing::South => 0.0,
        HorizontalFacing::West => 90.0,
        HorizontalFacing::North => 180.0,
        HorizontalFacing::East => 270.0,
    }
}
