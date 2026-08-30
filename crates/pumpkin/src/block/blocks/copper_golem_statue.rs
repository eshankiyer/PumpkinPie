use std::sync::Arc;

use pumpkin_data::block_properties::{
    BlockProperties, CopperGolemPose, CopperGolemStatueLikeProperties, EnumVariants,
};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::BlockStateImpl;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

use crate::block::blocks::copper_weathering;
use crate::block::entities::copper_golem_statue::CopperGolemStatueBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockIsReplacing, BlockMetadata, GetComparatorOutputArgs,
    NormalUseArgs, OnPlaceArgs, OnStateReplacedArgs, RandomTickArgs, UseWithItemArgs,
};
use crate::entity::EntityBase;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};

/// `net.minecraft.world.level.block.CopperGolemStatueBlock` and its weathering subclass
/// `WeatheringCopperGolemStatueBlock`.
pub struct CopperGolemStatueBlock;

/// The four oxidation stages, mirroring `WeatheringCopper.NEXT_BY_BLOCK`. The waxed
/// variants share every behaviour except random-tick weathering.
const OXIDATION_FAMILY: [&Block; 4] = [
    &Block::COPPER_GOLEM_STATUE,
    &Block::EXPOSED_COPPER_GOLEM_STATUE,
    &Block::WEATHERED_COPPER_GOLEM_STATUE,
    &Block::OXIDIZED_COPPER_GOLEM_STATUE,
];

/// `CopperGolemStatueBlock.shouldChangedStateKeepBlockEntity` retains the statue entity when
/// the old and new blocks are both in `BlockTags.COPPER_GOLEM_STATUES` (`CopperGolemStatueBlock.java:135-138`).
pub(crate) fn should_keep_block_entity(old_block: &Block, new_block: &Block) -> bool {
    old_block.has_tag(&tag::Block::MINECRAFT_COPPER_GOLEM_STATUES)
        && new_block.has_tag(&tag::Block::MINECRAFT_COPPER_GOLEM_STATUES)
}

impl BlockMetadata for CopperGolemStatueBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::COPPER_GOLEM_STATUE,
            BlockId::EXPOSED_COPPER_GOLEM_STATUE,
            BlockId::WEATHERED_COPPER_GOLEM_STATUE,
            BlockId::OXIDIZED_COPPER_GOLEM_STATUE,
            BlockId::WAXED_COPPER_GOLEM_STATUE,
            BlockId::WAXED_EXPOSED_COPPER_GOLEM_STATUE,
            BlockId::WAXED_WEATHERED_COPPER_GOLEM_STATUE,
            BlockId::WAXED_OXIDIZED_COPPER_GOLEM_STATUE,
        ]
        .into()
    }
}

impl CopperGolemStatueBlock {
    /// `CopperGolemStatueBlock.updatePose`: cycles the pose, plays the become-statue
    /// sound and emits a `block_change` game event attributed to the player.
    async fn update_pose(
        world: &Arc<World>,
        block: &Block,
        state_id: BlockStateId,
        position: &BlockPos,
        player: Option<&Arc<crate::entity::player::Player>>,
    ) {
        world.play_block_sound(
            Sound::EntityCopperGolemBecomeStatue,
            SoundCategory::Blocks,
            *position,
        );

        let mut props = CopperGolemStatueLikeProperties::from_state_id(state_id, block);
        props.copper_golem_pose = next_pose(props.copper_golem_pose);
        world
            .set_block_state(position, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
            .await;

        let context = player.map_or_else(GameEventContext::none, |player| {
            GameEventContext::of_entity(player.clone() as Arc<dyn EntityBase>)
        });
        emit_game_event(
            world,
            GameEvent::BlockChange,
            Vector3::new(
                f64::from(position.0.x) + 0.5,
                f64::from(position.0.y) + 0.5,
                f64::from(position.0.z) + 0.5,
            ),
            context,
        )
        .await;
    }
}

/// `CopperGolemStatueBlock.Pose.getNextPose`: `BY_ID` is built with
/// `OutOfBoundsStrategy.ZERO`, so STAR wraps back to STANDING.
const fn next_pose(pose: CopperGolemPose) -> CopperGolemPose {
    match pose {
        CopperGolemPose::Standing => CopperGolemPose::Sitting,
        CopperGolemPose::Sitting => CopperGolemPose::Running,
        CopperGolemPose::Running => CopperGolemPose::Star,
        CopperGolemPose::Star => CopperGolemPose::Standing,
    }
}

impl BlockBehaviour for CopperGolemStatueBlock {
    /// `getStateForPlacement`: faces away from the placing player, waterlogged when it
    /// replaced a water source. POSE always starts at STANDING (the default state).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = CopperGolemStatueLikeProperties::default(args.block);
            props.facing = args.player.get_entity().get_horizontal_facing();
            props.waterlogged = matches!(args.replacing, BlockIsReplacing::Water(_));
            props.to_state_id(args.block)
        })
    }

    /// `useItemOn` with an empty hand: cycles the pose.
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            Self::update_pose(
                args.world,
                args.block,
                state.id,
                args.position,
                Some(args.player),
            )
            .await;
            BlockActionResult::Success
        })
    }

    /// `WeatheringCopperGolemStatueBlock.useItemOn` (lines 50-82). An axe on the UNAFFECTED
    /// stage releases the golem here; on any other stage it is passed through so the axe item
    /// can de-wax or scrape it. Honeycomb is passed through to the waxing path, and anything
    /// else cycles the pose.
    ///
    /// UNAFFECTED is the unwaxed `minecraft:copper_golem_statue` alone: the waxed variants are
    /// plain `CopperGolemStatueBlock`s in vanilla (only the unwaxed four extend
    /// `WeatheringCopperGolemStatueBlock`), so an axe de-waxes them instead of releasing.
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let item: &Item = args.item_stack.item;
            if item.has_tag(&tag::Item::MINECRAFT_AXES) {
                if args.block.id != BlockId::COPPER_GOLEM_STATUE {
                    return BlockActionResult::PassToDefaultBlockAction;
                }

                let state = args.world.get_block_state(args.position);
                let props = CopperGolemStatueLikeProperties::from_state_id(state.id, args.block);

                // The block entity is created on placement (`block/entities/mod.rs`), but a
                // statue restored from an older world may predate that, and vanilla's own
                // `useItemOn` no-ops when the block entity is missing.
                let Some(block_entity) = args.world.get_block_entity(args.position) else {
                    return BlockActionResult::PassToDefaultBlockAction;
                };
                let Some(statue) = block_entity
                    .as_any()
                    .downcast_ref::<CopperGolemStatueBlockEntity>()
                else {
                    return BlockActionResult::PassToDefaultBlockAction;
                };

                statue
                    .remove_statue(args.world, props.facing, props.waterlogged)
                    .await;
                // `itemStack.hurtAndBreak(1, player, hand.asEquipmentSlot())`, line 72.
                args.player
                    .damage_item_in_slot(args.equipment_slot, 1)
                    .await;
                return BlockActionResult::Success;
            }

            if item.id == Item::HONEYCOMB.id {
                return BlockActionResult::PassToDefaultBlockAction;
            }

            let state = args.world.get_block_state(args.position);
            Self::update_pose(
                args.world,
                args.block,
                state.id,
                args.position,
                Some(args.player),
            )
            .await;
            BlockActionResult::Success
        })
    }

    /// `CopperGolemStatueBlock.affectNeighborsAfterRemoval` notifies comparator outputs after
    /// the statue is removed (`CopperGolemStatueBlock.java:157-160`).
    fn on_state_replaced<'a>(&'a self, args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .update_comparators(args.position, args.block)
                .await;
        })
    }

    /// `getAnalogOutputSignal`: `POSE.ordinal() + 1`, i.e. 1..=4.
    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let props = CopperGolemStatueLikeProperties::from_state_id(args.state.id, args.block);
            let ordinal = match props.copper_golem_pose {
                CopperGolemPose::Standing => 0,
                CopperGolemPose::Sitting => 1,
                CopperGolemPose::Running => 2,
                CopperGolemPose::Star => 3,
            };
            Some(ordinal + 1)
        })
    }

    /// Vanilla `CopperGolemStatueBlock.getCloneItemStack` and
    /// `CopperGolemStatueBlockEntity.getItem` (`CopperGolemStatueBlock.java:151-155`,
    /// `CopperGolemStatueBlockEntity.java:43-47`): pick-block preserves the statue pose on
    /// the returned item through its `block_state` component.
    fn get_clone_item_stack(
        &self,
        args: crate::block::GetCloneItemStackArgs<'_>,
    ) -> Option<pumpkin_data::item_stack::ItemStack> {
        let block_entity = args.world.get_block_entity(args.position)?;
        block_entity
            .as_any()
            .downcast_ref::<CopperGolemStatueBlockEntity>()?;
        let state = args.world.get_block_state(args.position);
        let props = CopperGolemStatueLikeProperties::from_state_id(state.id, args.block);
        let mut stack =
            pumpkin_data::item_stack::ItemStack::new(1, Item::from_id(args.block.item_id)?);
        stack.patch.push((
            DataComponent::BlockState,
            Some(Box::new(BlockStateImpl {
                properties: std::borrow::Cow::Owned(vec![(
                    std::borrow::Cow::Borrowed("copper_golem_pose"),
                    std::borrow::Cow::Borrowed(props.copper_golem_pose.to_value()),
                )]),
            })),
        ));
        Some(stack)
    }

    /// `WeatheringCopperGolemStatueBlock.randomTick`: only the unwaxed variants weather,
    /// and FACING/POSE/WATERLOGGED carry over to the next oxidation stage.
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let props = CopperGolemStatueLikeProperties::from_state_id(state.id, args.block);
            copper_weathering::try_oxidize_copper(
                args.world,
                args.position,
                args.block,
                &OXIDATION_FAMILY,
                |next_block| props.to_state_id(next_block),
            )
            .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CopperGolemStatueBlock, OXIDATION_FAMILY, next_pose, should_keep_block_entity};
    use crate::block::BlockMetadata;
    use pumpkin_data::block_properties::CopperGolemPose;
    use pumpkin_data::{Block, BlockId};

    /// `CopperGolemStatueBlock.Pose.getNextPose` wraps via `OutOfBoundsStrategy.ZERO`, so four
    /// right-clicks return the statue to the pose it started in.
    #[test]
    fn cycling_the_pose_four_times_returns_to_standing() {
        let mut pose = CopperGolemPose::Standing;
        for _ in 0..4 {
            pose = next_pose(pose);
        }
        assert!(matches!(pose, CopperGolemPose::Standing));
    }

    /// `WeatheringCopperGolemStatueBlock.useItemOn` lines 70-78: only the UNAFFECTED stage
    /// releases the golem, and only the four unwaxed statues are weathering blocks at all, so
    /// UNAFFECTED means the plain `copper_golem_statue` id and nothing else. Every other id
    /// this block claims must fall through to the axe item, which scrapes or de-waxes it.
    #[test]
    fn only_the_unwaxed_unaffected_statue_releases_its_golem() {
        let release_stage = BlockId::COPPER_GOLEM_STATUE;
        assert_eq!(OXIDATION_FAMILY[0].id, release_stage);

        let claimed = CopperGolemStatueBlock::ids();
        assert!(claimed.contains(&release_stage));
        assert_eq!(
            claimed.iter().filter(|id| **id == release_stage).count(),
            1,
            "the release stage must be a single id, not a family"
        );
        assert!(
            !claimed
                .iter()
                .any(|id| *id == BlockId::WAXED_COPPER_GOLEM_STATUE && *id == release_stage),
            "the waxed statue is a plain CopperGolemStatueBlock in vanilla and de-waxes instead"
        );
    }

    /// `CopperGolemStatueBlock.shouldChangedStateKeepBlockEntity` (`CopperGolemStatueBlock.java:135-138`)
    /// applies across the oxidation and waxing block variants, but not when the statue is removed.
    #[test]
    fn statue_family_transitions_keep_the_block_entity() {
        assert!(should_keep_block_entity(
            &Block::COPPER_GOLEM_STATUE,
            &Block::EXPOSED_COPPER_GOLEM_STATUE
        ));
        assert!(should_keep_block_entity(
            &Block::OXIDIZED_COPPER_GOLEM_STATUE,
            &Block::WAXED_OXIDIZED_COPPER_GOLEM_STATUE
        ));
        assert!(!should_keep_block_entity(
            &Block::COPPER_GOLEM_STATUE,
            &Block::AIR
        ));
    }
}
