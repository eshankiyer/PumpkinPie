use std::sync::Arc;

use crate::entity::{
    Entity,
    mob::{Mob, MobEntity, slime::SlimeEntity},
};
use crate::world::World;

/// Wraps `SlimeEntity`, mirroring how `MagmaCube.java` extends `AbstractCubeMob` the same
/// way `Slime.java` does.
pub struct MagmaCubeEntity {
    pub slime: Arc<SlimeEntity>,
}

impl MagmaCubeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let slime = SlimeEntity::new(entity);
        Arc::new(Self { slime })
    }
}


impl Mob for MagmaCubeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        self.slime.get_mob_entity()
    }

    fn light_level_dependent_magic_value(&self, _world: &World) -> f32 {
        1.0
    }

    fn mob_init_data_tracker(&self) {
        self.slime.mob_init_data_tracker();
    }

    fn mob_tick(&self, caller: &dyn crate::entity::EntityBase) {
        self.slime.mob_tick(caller);
    }

    fn post_tick(&self) {
        self.slime.post_tick();
    }

    fn mob_player_collision(&self, player: &Arc<crate::entity::player::Player>) {
        self.slime.mob_player_collision(player);
    }
}
