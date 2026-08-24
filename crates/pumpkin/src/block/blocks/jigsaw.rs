use std::sync::Arc;

use crate::block::entities::jigsaw_block::JigsawBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, CanPlaceAtArgs, NormalUseArgs, OnPlaceArgs, PlacedArgs,
};
use crate::entity::EntityBase;
use pumpkin_data::block_properties::{
    BlockProperties, HorizontalFacing, JigsawLikeProperties, Orientation,
};
use pumpkin_data::block_rotation::{Mirror, Rotation};
use pumpkin_data::{BlockDirection, BlockStateId};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::{GameMode, PermissionLvl};

use pumpkin_world::generation::structure::structures::jigsaw::JigsawJointType;

#[pumpkin_block("minecraft:jigsaw")]
pub struct JigsawBlock;

impl JigsawBlock {
    #[must_use]
    pub const fn from_front_top(front: BlockDirection, top: BlockDirection) -> Orientation {
        match (front, top) {
            (BlockDirection::Down, BlockDirection::East) => Orientation::DownEast,
            (BlockDirection::Down, BlockDirection::North) => Orientation::DownNorth,
            (BlockDirection::Down, BlockDirection::South) => Orientation::DownSouth,
            (BlockDirection::Down, BlockDirection::West) => Orientation::DownWest,
            (BlockDirection::Up, BlockDirection::East) => Orientation::UpEast,
            (BlockDirection::Up, BlockDirection::North) => Orientation::UpNorth,
            (BlockDirection::Up, BlockDirection::South) => Orientation::UpSouth,
            (BlockDirection::Up, BlockDirection::West) => Orientation::UpWest,
            (BlockDirection::West, BlockDirection::Up) => Orientation::WestUp,
            (BlockDirection::East, BlockDirection::Up) => Orientation::EastUp,
            (BlockDirection::South, BlockDirection::Up) => Orientation::SouthUp,
            _ => Orientation::NorthUp, // Default
        }
    }

    #[must_use]
    pub const fn to_front_top(orientation: Orientation) -> (BlockDirection, BlockDirection) {
        match orientation {
            Orientation::DownEast => (BlockDirection::Down, BlockDirection::East),
            Orientation::DownNorth => (BlockDirection::Down, BlockDirection::North),
            Orientation::DownSouth => (BlockDirection::Down, BlockDirection::South),
            Orientation::DownWest => (BlockDirection::Down, BlockDirection::West),
            Orientation::UpEast => (BlockDirection::Up, BlockDirection::East),
            Orientation::UpNorth => (BlockDirection::Up, BlockDirection::North),
            Orientation::UpSouth => (BlockDirection::Up, BlockDirection::South),
            Orientation::UpWest => (BlockDirection::Up, BlockDirection::West),
            Orientation::WestUp => (BlockDirection::West, BlockDirection::Up),
            Orientation::EastUp => (BlockDirection::East, BlockDirection::Up),
            Orientation::NorthUp => (BlockDirection::North, BlockDirection::Up),
            Orientation::SouthUp => (BlockDirection::South, BlockDirection::Up),
        }
    }

    #[must_use]
    pub const fn get_front_facing(orientation: Orientation) -> BlockDirection {
        Self::to_front_top(orientation).0
    }

    #[must_use]
    pub const fn get_top_facing(orientation: Orientation) -> BlockDirection {
        Self::to_front_top(orientation).1
    }

    #[must_use]
    pub fn get_front_facing_from_state(
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
    ) -> BlockDirection {
        let props = JigsawLikeProperties::from_state_id(state_id, block);
        Self::get_front_facing(props.r#orientation)
    }

    #[must_use]
    pub fn get_top_facing_from_state(
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
    ) -> BlockDirection {
        let props = JigsawLikeProperties::from_state_id(state_id, block);
        Self::get_top_facing(props.r#orientation)
    }

    #[must_use]
    pub fn can_attach(
        source_front: BlockDirection,
        source_top: BlockDirection,
        source_joint: JigsawJointType,
        source_target: &str,
        target_front: BlockDirection,
        target_top: BlockDirection,
        target_name: &str,
    ) -> bool {
        let rollable = source_joint == JigsawJointType::Rollable;
        source_front == target_front.opposite()
            && (rollable || source_top == target_top)
            && source_target == target_name
    }
}

impl BlockBehaviour for JigsawBlock {
    /// Vanilla `GameMasterBlockItem.getPlacementState` (GameMasterBlockItem.java:15-18): the
    /// jigsaw item is a `GameMasterBlockItem`, so a player who cannot use game-master blocks
    /// gets no placement state and the block is never placed.
    /// `Player.canUseGameMasterBlocks` (Player.java:1863-1865) requires instabuild plus
    /// permission level 2; Pumpkin models instabuild through creative mode (the same mapping
    /// `CommandBlock::can_place_at` uses). A `None` player context (no player involved in the
    /// placement) passes, matching vanilla's null-player branch of the same check.
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let Some(player) = args.player else {
            return true;
        };
        player.gamemode.load() == GameMode::Creative
            && player.permission_lvl.load() >= PermissionLvl::Two
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = JigsawLikeProperties::default(args.block);
            let front = args.direction;
            let top = if front == BlockDirection::Up || front == BlockDirection::Down {
                horizontal_facing_to_dir(args.player.get_entity().get_horizontal_facing())
                    .opposite()
            } else {
                BlockDirection::Up
            };

            props.r#orientation = Self::from_front_top(front, top);
            props.to_state_id(args.block)
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            if args.player.permission_lvl.load() < PermissionLvl::Two {
                return BlockActionResult::Pass;
            }
            if args.player.gamemode.load() != GameMode::Creative {
                return BlockActionResult::Pass;
            }
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return BlockActionResult::Pass;
            };
            args.world.update_block_entity(&block_entity);
            BlockActionResult::SuccessServer
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = JigsawBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(entity));
        })
    }

    fn mirror(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        mirror: Mirror,
    ) -> &'static pumpkin_data::BlockState {
        if mirror == Mirror::None {
            return pumpkin_data::BlockState::from_id(state_id);
        }
        let mut props = JigsawLikeProperties::from_state_id(state_id, block);
        let (front, top) = Self::to_front_top(props.r#orientation);

        let new_front = mirror_direction(front, mirror);
        let new_top = mirror_direction(top, mirror);

        props.r#orientation = Self::from_front_top(new_front, new_top);
        pumpkin_data::BlockState::from_id(props.to_state_id(block))
    }

    fn rotate(
        &self,
        block: &pumpkin_data::Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static pumpkin_data::BlockState {
        if rotation == Rotation::None {
            return pumpkin_data::BlockState::from_id(state_id);
        }
        let mut props = JigsawLikeProperties::from_state_id(state_id, block);
        let (front, top) = Self::to_front_top(props.r#orientation);

        let new_front = rotate_direction(front, rotation);
        let new_top = rotate_direction(top, rotation);

        props.r#orientation = Self::from_front_top(new_front, new_top);
        pumpkin_data::BlockState::from_id(props.to_state_id(block))
    }
}

const fn horizontal_facing_to_dir(facing: HorizontalFacing) -> BlockDirection {
    match facing {
        HorizontalFacing::North => BlockDirection::North,
        HorizontalFacing::South => BlockDirection::South,
        HorizontalFacing::West => BlockDirection::West,
        HorizontalFacing::East => BlockDirection::East,
    }
}

fn rotate_direction(dir: BlockDirection, rotation: Rotation) -> BlockDirection {
    if dir == BlockDirection::Up || dir == BlockDirection::Down {
        return dir;
    }
    match rotation {
        Rotation::None => dir,
        Rotation::Clockwise90 => match dir {
            BlockDirection::North => BlockDirection::East,
            BlockDirection::East => BlockDirection::South,
            BlockDirection::South => BlockDirection::West,
            BlockDirection::West => BlockDirection::North,
            _ => dir,
        },
        Rotation::Rotate180 => match dir {
            BlockDirection::North => BlockDirection::South,
            BlockDirection::South => BlockDirection::North,
            BlockDirection::East => BlockDirection::West,
            BlockDirection::West => BlockDirection::East,
            _ => dir,
        },
        Rotation::CounterClockwise90 => match dir {
            BlockDirection::North => BlockDirection::West,
            BlockDirection::West => BlockDirection::South,
            BlockDirection::South => BlockDirection::East,
            BlockDirection::East => BlockDirection::North,
            _ => dir,
        },
    }
}

/// Matches vanilla `Mirror.rotation()`: `LEFT_RIGHT` is `OctahedralGroup.INVERT_Z`
/// (flips north/south), `FRONT_BACK` is `OctahedralGroup.INVERT_X` (flips east/west).
const fn mirror_direction(dir: BlockDirection, mirror: Mirror) -> BlockDirection {
    match mirror {
        Mirror::None => dir,
        Mirror::LeftRight => match dir {
            BlockDirection::North => BlockDirection::South,
            BlockDirection::South => BlockDirection::North,
            _ => dir,
        },
        Mirror::FrontBack => match dir {
            BlockDirection::East => BlockDirection::West,
            BlockDirection::West => BlockDirection::East,
            _ => dir,
        },
    }
}

#[cfg(test)]
mod mirror_direction_tests {
    use super::*;

    #[test]
    fn left_right_flips_north_south() {
        assert_eq!(
            mirror_direction(BlockDirection::North, Mirror::LeftRight),
            BlockDirection::South
        );
        assert_eq!(
            mirror_direction(BlockDirection::South, Mirror::LeftRight),
            BlockDirection::North
        );
        assert_eq!(
            mirror_direction(BlockDirection::East, Mirror::LeftRight),
            BlockDirection::East
        );
    }

    #[test]
    fn front_back_flips_east_west() {
        assert_eq!(
            mirror_direction(BlockDirection::East, Mirror::FrontBack),
            BlockDirection::West
        );
        assert_eq!(
            mirror_direction(BlockDirection::West, Mirror::FrontBack),
            BlockDirection::East
        );
        assert_eq!(
            mirror_direction(BlockDirection::North, Mirror::FrontBack),
            BlockDirection::North
        );
    }

    #[test]
    fn none_is_identity() {
        for dir in [
            BlockDirection::North,
            BlockDirection::South,
            BlockDirection::East,
            BlockDirection::West,
            BlockDirection::Up,
            BlockDirection::Down,
        ] {
            assert_eq!(mirror_direction(dir, Mirror::None), dir);
        }
    }
}
