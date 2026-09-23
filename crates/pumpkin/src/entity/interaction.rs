use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

use pumpkin_data::{damage::DamageType, item_stack::ItemStack};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::{
    boundingbox::{BoundingBox, EntityDimensions},
    vector3::Vector3,
};

use crate::{
    entity::{Entity, EntityBase, NBTStorage, living::LivingEntity, player::Player},
    server::Server,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerAction {
    pub player: Uuid,
    pub timestamp: i64,
}

impl PlayerAction {
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_uuid("player", self.player);
        compound.put_long("timestamp", self.timestamp);
        compound
    }

    #[must_use]
    pub fn from_nbt(compound: &NbtCompound) -> Option<Self> {
        let player = compound.get_uuid("player")?;
        let timestamp = compound.get_long("timestamp")?;
        Some(Self { player, timestamp })
    }
}

pub struct InteractionEntity {
    pub entity: Entity,
    pub width: Mutex<f32>,
    pub height: Mutex<f32>,
    pub response: AtomicBool,
    pub attack: Mutex<Option<PlayerAction>>,
    pub interaction: Mutex<Option<PlayerAction>>,
}

impl InteractionEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        let width = 1.0;
        let height = 1.0;
        let dimensions = EntityDimensions::new(width, height, height * 0.85);
        let pos = entity.pos.load();
        let aabb = BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions);
        entity.bounding_box.store(aabb);

        Arc::new(Self {
            entity,
            width: Mutex::new(width),
            height: Mutex::new(height),
            response: AtomicBool::new(false),
            attack: Mutex::new(None),
            interaction: Mutex::new(None),
        })
    }

    pub fn get_width(&self) -> f32 {
        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_width(&self, width: f32) {
        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = width;
        self.update_dimensions();
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::interaction::WIDTH,
                width,
            )],
            None,
        );
    }

    pub fn get_height(&self) -> f32 {
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_height(&self, height: f32) {
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = height;
        self.update_dimensions();
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::interaction::HEIGHT,
                height,
            )],
            None,
        );
    }

    pub fn get_response(&self) -> bool {
        self.response.load(Ordering::Relaxed)
    }

    pub fn set_response(&self, response: bool) {
        self.response.store(response, Ordering::Relaxed);
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::interaction::RESPONSE,
                response,
            )],
            None,
        );
    }

    pub fn update_dimensions(&self) {
        let width = *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let height = *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dimensions = EntityDimensions::new(width, height, height * 0.85);
        let pos = self.entity.pos.load();
        let aabb = BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions);
        self.entity.bounding_box.store(aabb);
        self.entity.base_dimension.store(dimensions);
        self.entity.entity_dimension.store(dimensions);
    }

    pub fn get_last_attacker(&self) -> Option<PlayerAction> {
        *self
            .attack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get_target(&self) -> Option<PlayerAction> {
        *self
            .interaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl NBTStorage for InteractionEntity {
    fn write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.write_nbt(nbt);
        nbt.put_float(
            "width",
            *self
                .width
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_float(
            "height",
            *self
                .height
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        nbt.put_bool("response", self.response.load(Ordering::Relaxed));

        let attack = *self
            .attack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(attack) = attack {
            nbt.put("attack", NbtTag::Compound(attack.to_nbt()));
        }

        let interaction = *self
            .interaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(interaction) = interaction {
            nbt.put("interaction", NbtTag::Compound(interaction.to_nbt()));
        }
    }

    fn read_nbt(&mut self, nbt: &mut NbtCompound) {
        self.read_nbt_non_mut(nbt);
    }

    fn read_nbt_non_mut(&self, nbt: &NbtCompound) {
        self.entity.read_nbt_non_mut(nbt);

        let width = nbt.get_float("width").unwrap_or(1.0);
        let height = nbt.get_float("height").unwrap_or(1.0);
        let response = nbt.get_bool("response").unwrap_or(false);

        *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = width;
        *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = height;
        self.response.store(response, Ordering::Relaxed);
        self.update_dimensions();

        if let Some(attack_compound) = nbt.get_compound("attack") {
            *self
                .attack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                PlayerAction::from_nbt(attack_compound);
        } else {
            *self
                .attack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }

        if let Some(interaction_compound) = nbt.get_compound("interaction") {
            *self
                .interaction
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                PlayerAction::from_nbt(interaction_compound);
        } else {
            *self
                .interaction
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }
}

impl EntityBase for InteractionEntity {
    /// `Interaction.isPickable` returns true; the `EntityBase` default is false, and an
    /// interaction entity that is not pickable cannot be clicked at all.
    fn is_pickable(&self) -> bool {
        true
    }

    fn tick(&self, _caller: &Arc<dyn EntityBase>, _server: &Server) {}

    fn init_data_tracker(&self) {
        let width = *self
            .width
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let height = *self
            .height
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let response = self.response.load(Ordering::Relaxed);

        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::interaction::WIDTH,
                width,
            )],
            None,
        );
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::interaction::HEIGHT,
                height,
            )],
            None,
        );
        self.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::interaction::RESPONSE,
                response,
            )],
            None,
        );
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn is_pushed_by_fluids(&self) -> bool {
        false
    }

    fn can_hit(&self) -> bool {
        true
    }

    fn is_immune_to_explosion(&self) -> bool {
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let player = source
            .or(cause)
            .and_then(|e| e.cast_any().downcast_ref::<Player>());
        if let Some(player) = player {
            let timestamp = self.entity.world.load().get_world_age();
            *self
                .attack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PlayerAction {
                player: player.gameprofile.id,
                timestamp,
            });
        }
        false
    }

    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        let timestamp = self.entity.world.load().get_world_age();
        *self
            .interaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PlayerAction {
            player: player.gameprofile.id,
            timestamp,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_util::math::boundingbox::EntityDimensions;

    /// `Interaction.getDimensions` builds `EntityDimensions.scalable(width, height)`, and
    /// `EntityDimensions.scalable` passes `fixed = false`, which is what makes eye height
    /// track height rather than staying constant.
    fn scaled(width: f32, height: f32) -> EntityDimensions {
        EntityDimensions::new(width, height, height * 0.85)
    }

    #[test]
    fn default_dimensions_are_one_by_one() {
        let dims = scaled(1.0, 1.0);
        assert_eq!(dims.width, 1.0);
        assert_eq!(dims.height, 1.0);
        assert_eq!(dims.eye_height, 0.85);
    }

    #[test]
    fn eye_height_tracks_height_at_vanilla_ratio() {
        let dims = scaled(2.0, 4.0);
        assert_eq!(dims.width, 2.0);
        assert_eq!(dims.height, 4.0);
        assert_eq!(dims.eye_height, 3.4);
    }
}
