use std::sync::{Arc, atomic::Ordering};

use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use crate::{
    block::entities::BlockEntity,
    world::{World, game_event},
};

pub use pumpkin_world::block::viewer::ViewerCountTracker;

pub trait ViewerCountTrackerExt {
    fn update_viewer_count<T>(&self, entity: &T, world: &Arc<World>, position: &BlockPos)
    where
        T: BlockEntity + ViewerCountListener + 'static;
}

impl ViewerCountTrackerExt for ViewerCountTracker {
    fn update_viewer_count<T>(&self, entity: &T, world: &Arc<World>, position: &BlockPos)
    where
        T: BlockEntity + ViewerCountListener + 'static,
    {
        let current = self.current.load(Ordering::Relaxed);
        let old = self.old.swap(current, Ordering::Relaxed);
        if old != current {
            // ContainerOpenersCounter.onViewerCountChange: fires GameEvent.CONTAINER_OPEN/
            // CONTAINER_CLOSE only on the 0<->nonzero transition, not on every viewer-count
            // change. Vanilla passes the opening/closing entity as context when known and
            // `null` otherwise (ContainerOpenersCounter.java:81,84); this tracker only has
            // the count, not which entity changed it, so `GameEventContext::none()` is used
            // -- the same fallback vanilla itself uses when no entity is known.
            let event_pos = Vector3::new(
                f64::from(position.0.x) + 0.5,
                f64::from(position.0.y) + 0.5,
                f64::from(position.0.z) + 0.5,
            );
            match (old, current) {
                (n, 0) if n > 0 => {
                    entity.on_container_close(world, position);
                    game_event::emit_game_event(
                        world,
                        pumpkin_data::game_event::GameEvent::ContainerClose,
                        event_pos,
                        game_event::GameEventContext::none(),
                    );
                }
                (0, n) if n > 0 => {
                    entity.on_container_open(world, position);
                    game_event::emit_game_event(
                        world,
                        pumpkin_data::game_event::GameEvent::ContainerOpen,
                        event_pos,
                        game_event::GameEventContext::none(),
                    );
                }
                _ => {} // Ignore
            }

            entity.on_viewer_count_update(world, position, old, current);
        }
    }
}

pub trait ViewerCountListener: Send + Sync {
    fn on_container_open(&self, _world: &Arc<World>, _position: &BlockPos) {}

    fn on_container_close(&self, _world: &Arc<World>, _position: &BlockPos) {}

    fn on_viewer_count_update(
        &self,
        _world: &Arc<World>,
        _position: &BlockPos,
        _old: u16,
        _new: u16,
    ) {
    }
}
