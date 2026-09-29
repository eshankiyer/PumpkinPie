use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, GetCloneItemStackArgs,
    GetStateForNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs, PlacedArgs,
};
use crate::entity::EntityBase;
use pumpkin_data::block_properties::{
    BlockProperties, WallTorchLikeProperties, WhiteBannerLikeProperties,
};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{BlockEntityDataImpl, DataComponentImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockDirection, BlockStateId, HorizontalFacingExt};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

use crate::block::entities::banner::BannerBlockEntity;
use crate::block::entities::custom_name_component;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use std::sync::Arc;

/// `BannerBlock` and `WallBannerBlock`, both of which extend `AbstractBannerBlock`
/// (`net/minecraft/world/level/block/AbstractBannerBlock.java`).
///
/// `minecraft:banners` holds all 32 ids, standing and wall alike, so a single behaviour has to
/// tell them apart the way `signs.rs` does. They are genuinely different blocks:
///
/// * a standing banner carries `rotation` (16 values) and survives on a solid block BELOW it
///   (`BannerBlock.java:53-55`);
/// * a wall banner carries `facing` (4 values, the generated data models it with
///   `WallTorchLikeProperties`) and survives on a solid block BEHIND it, i.e. at
///   `pos.relative(facing.getOpposite())` (`WallBannerBlock.java:39-42`).
///
/// Treating both as standing banners meant a wall banner was placed by writing a 16-value
/// `rotation` into a 4-value state and was held up by whatever sat under it rather than by the
/// wall it hangs on.
#[pumpkin_block_from_tag("minecraft:banners")]
pub struct BannerBlock;

/// Wall banners are exactly the ids whose registry name ends in `wall_banner`; the same
/// name test `signs.rs` uses to separate wall signs from standing ones.
fn is_wall_banner(block: &Block) -> bool {
    block.name.ends_with("wall_banner")
}

/// `WallBannerBlock#canSurvive` (WallBannerBlock.java:39-42): the support is the block the banner
/// faces away from.
fn wall_support_direction(block: &Block, state_id: BlockStateId) -> BlockDirection {
    WallTorchLikeProperties::from_state_id(state_id, block)
        .facing
        .to_block_direction()
        .opposite()
}

impl BlockBehaviour for BannerBlock {
    /// Vanilla `AbstractBannerBlock.getCloneItemStack` (`AbstractBannerBlock.java:34-37`) calls
    /// `BannerBlockEntity.getItem` (`BannerBlockEntity.java:74-81`), which applies
    /// `collectComponents` (`BannerBlockEntity.java:96-100`) in every pick mode: the item carries
    /// `BANNER_PATTERNS` and `CUSTOM_NAME`.
    fn get_clone_item_stack(&self, args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        let item = Item::from_id(args.block.item_id)?;
        let entity = args.world.get_block_entity(args.position)?;
        let banner = entity.as_any().downcast_ref::<BannerBlockEntity>()?;
        let custom_name = banner
            .custom_name
            .try_lock()
            .ok()
            .and_then(|name| name.clone());
        Some(banner_item_stack(item, banner.get_patterns(), custom_name))
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = BannerBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(entity));
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if is_wall_banner(args.block) {
                // `WallBannerBlock#getStateForPlacement` (WallBannerBlock.java:66-83) faces the
                // banner away from the surface it was placed against.
                let mut props = WallTorchLikeProperties::default(args.block);
                if let Some(facing) = args.direction.opposite().to_horizontal_facing() {
                    props.facing = facing;
                }
                return props.to_state_id(args.block);
            }

            // `BannerBlock#getStateForPlacement` (BannerBlock.java:57-60).
            let mut props = WhiteBannerLikeProperties::default(args.block);
            props.rotation = args.player.get_entity().get_flipped_rotation_16();
            props.to_state_id(args.block)
        })
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        if !is_wall_banner(args.block) {
            return args
                .block_accessor
                .get_block_state(&args.position.down())
                .is_solid();
        }

        // `WallBannerBlock#getStateForPlacement` (WallBannerBlock.java:66-83) tests `canSurvive`
        // against the facing it is about to set, not against the block's current state. During
        // placement `args.state` is still the default state, so the placement direction is the
        // only thing that says which wall the banner will hang on; `on_place` above sets
        // `facing = direction.opposite()`, and vanilla's support is `facing.getOpposite()`, so the
        // support sits exactly one step along `direction`. Neighbour updates and scheduled ticks
        // pass no direction and a real state, and fall through to the state-derived facing.
        let support_direction = match args.direction {
            Some(direction) if direction.to_horizontal_facing().is_some() => direction,
            _ => wall_support_direction(args.block, args.state.id),
        };

        args.block_accessor
            .get_block_state(&args.position.offset(support_direction.to_offset()))
            .is_solid()
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            if !can_survive(args.world.as_ref(), args.block, state_id, args.position) {
                args.world
                    .break_block(args.position, None, BlockFlags::empty())
                    .await;
            }
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if !can_survive(args.world, args.block, args.state_id, args.position) {
                args.world
                    .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            }
            args.state_id
        })
    }
}

/// The item `BannerBlockEntity.getItem` builds. `BannerPatternsImpl` has no payload yet, so the
/// layers travel as a raw `patterns` list in `BLOCK_ENTITY_DATA`; the name is the `CUSTOM_NAME`
/// component, which placement reads instead of a raw `CustomName` (`BannerBlockEntity.java:89-93`).
fn banner_item_stack(
    item: &'static Item,
    patterns: Option<Vec<NbtTag>>,
    custom_name: Option<String>,
) -> ItemStack {
    let mut components = Vec::new();
    if let Some(patterns) = patterns {
        let mut nbt = NbtCompound::new();
        nbt.put_list("patterns", patterns);
        components.push((
            DataComponent::BlockEntityData,
            Some(Box::new(BlockEntityDataImpl { nbt }).to_dyn()),
        ));
    }
    if let Some(name) = custom_name {
        components.push(custom_name_component(name));
    }
    if components.is_empty() {
        ItemStack::new(1, item)
    } else {
        ItemStack::new_with_component(1, item, components)
    }
}

/// `AbstractBannerBlock` has no `canSurvive` of its own; the two subclasses each define one.
fn can_survive(
    world: &dyn BlockAccessor,
    block: &Block,
    state_id: BlockStateId,
    position: &BlockPos,
) -> bool {
    let support = if is_wall_banner(block) {
        position.offset(wall_support_direction(block, state_id).to_offset())
    } else {
        position.down()
    };

    world.get_block_state(&support).is_solid()
}

#[cfg(test)]
mod test {
    use super::{banner_item_stack, is_wall_banner, wall_support_direction};
    use crate::block::entities::apply_components_from_item_stack;
    use crate::block::entities::banner::BannerBlockEntity;
    use pumpkin_data::block_properties::{
        BlockProperties, HorizontalFacing, WallTorchLikeProperties,
    };
    use pumpkin_data::data_component_impl::{BlockEntityDataImpl, CustomNameImpl};
    use pumpkin_data::item::Item;
    use pumpkin_data::{Block, BlockDirection};
    use pumpkin_nbt::tag::NbtTag;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::text::TextComponent;

    #[test]
    fn standing_and_wall_banners_are_told_apart() {
        assert!(is_wall_banner(&Block::WHITE_WALL_BANNER));
        assert!(is_wall_banner(&Block::BLACK_WALL_BANNER));
        assert!(!is_wall_banner(&Block::WHITE_BANNER));
        assert!(!is_wall_banner(&Block::BLACK_BANNER));
    }

    /// `WallBannerBlock.java:41` looks at `pos.relative(FACING.getOpposite())`, so a banner facing
    /// north hangs on the block to its south.
    #[test]
    fn wall_banner_support_is_behind_it() {
        for (facing, expected) in [
            (HorizontalFacing::North, BlockDirection::South),
            (HorizontalFacing::South, BlockDirection::North),
            (HorizontalFacing::East, BlockDirection::West),
            (HorizontalFacing::West, BlockDirection::East),
        ] {
            let mut props = WallTorchLikeProperties::default(&Block::WHITE_WALL_BANNER);
            props.facing = facing;
            let state_id = props.to_state_id(&Block::WHITE_WALL_BANNER);
            assert_eq!(
                wall_support_direction(&Block::WHITE_WALL_BANNER, state_id),
                expected
            );
        }
    }

    /// A wall banner has four states, one per facing; the sixteen-value `rotation` the standing
    /// banner uses does not fit in them, which is what the old shared code wrote there.
    #[test]
    fn wall_banner_has_four_states_not_sixteen() {
        assert_eq!(Block::WHITE_WALL_BANNER.states.len(), 4);
        assert_eq!(Block::WHITE_BANNER.states.len(), 16);
    }

    /// `AbstractBannerBlock.getCloneItemStack` returns `BannerBlockEntity.getItem` in every pick
    /// mode, so a plain middle-click item carries `CUSTOM_NAME` as a component and placing it must
    /// keep the name (`BannerBlockEntity.java:89-100`).
    #[tokio::test]
    async fn picked_banner_keeps_its_name_and_patterns_when_placed() {
        let layers = vec![NbtTag::String("stripe".into())];
        let stack = banner_item_stack(
            &Item::WHITE_BANNER,
            Some(layers.clone()),
            Some("\"Flag\"".to_string()),
        );

        let name = stack
            .get_data_component::<CustomNameImpl>()
            .expect("the picked banner should carry CUSTOM_NAME");
        assert_eq!(name.name.clone().get_text(), "Flag");
        let data = stack
            .get_data_component::<BlockEntityDataImpl>()
            .expect("the picked banner should carry its layers");
        assert!(!data.nbt.has("CustomName"));

        let placed = apply_components_from_item_stack(
            &BannerBlockEntity::new(BlockPos::new(0, 64, 0)),
            &stack,
        )
        .expect("placing the picked banner should rebuild the entity");
        let placed = placed
            .as_any()
            .downcast_ref::<BannerBlockEntity>()
            .expect("component application should preserve the banner type");
        let stored = placed
            .custom_name
            .lock()
            .await
            .clone()
            .expect("the placed banner should keep its name");
        let stored = serde_json::from_str::<TextComponent>(&stored)
            .expect("the stored name should be a text component");
        assert_eq!(stored.get_text(), "Flag");
        assert_eq!(placed.get_patterns(), Some(layers));
    }

    #[test]
    fn plain_banner_pick_has_no_components() {
        let stack = banner_item_stack(&Item::WHITE_BANNER, None, None);
        assert!(stack.patch.is_empty());
    }
}
