//! Vanilla per-type entity attachment points.
//!
//! These come from `EntityType.Builder.passengerAttachments`, `ridingOffset`,
//! `vehicleAttachment` and `attach` in `EntityTypes.java`. The extracted entity data carries no
//! attachments, so the table is transcribed from the builder calls; every type not listed uses
//! only the fallbacks.

use pumpkin_data::entity::EntityType;
use pumpkin_util::math::{
    boundingbox::{EntityAttachmentsBuilder, EntityDimensions},
    vector3::Vector3,
};

const fn b() -> EntityAttachmentsBuilder {
    EntityAttachmentsBuilder::new()
}

/// Zombie-shaped humanoids: `passengerAttachments(y).ridingOffset(-0.7F)`.
const fn zombie_like(passenger_y: f32) -> EntityAttachmentsBuilder {
    b().passenger_y(passenger_y).riding_offset(-0.7)
}

/// Illagers: `passengerAttachments(2.0F).ridingOffset(-0.6F)`.
const ILLAGER: EntityAttachmentsBuilder = b().passenger_y(2.0).riding_offset(-0.6);
/// Skeleton-shaped mobs: `ridingOffset(-0.7F)`.
const SKELETON: EntityAttachmentsBuilder = b().riding_offset(-0.7);
/// Every minecart: `passengerAttachments(0.1875F)`.
const MINECART: EntityAttachmentsBuilder = b().passenger_y(0.1875);

/// `(type id, attachments)` rows in `EntityTypes.java` order.
const TYPE_ATTACHMENTS: &[(u16, EntityAttachmentsBuilder)] = &[
    // EntityTypes.java:167
    (EntityType::ALLAY.id, b().riding_offset(0.04)),
    // EntityTypes.java:239
    (EntityType::BOGGED.id, SKELETON),
    // EntityTypes.java:262
    (EntityType::CAT.id, b().passenger_y(0.5125)),
    // EntityTypes.java:286
    (EntityType::CHEST_MINECART.id, MINECART),
    // EntityTypes.java:293
    (
        EntityType::CHICKEN.id,
        b().passenger(Vector3::new(0.0, 0.7, -0.1)),
    ),
    // EntityTypes.java:304
    (EntityType::COMMAND_BLOCK_MINECART.id, MINECART),
    // EntityTypes.java:308
    (EntityType::COW.id, b().passenger_y(1.36875)),
    // EntityTypes.java:338
    (EntityType::DONKEY.id, b().passenger_y(1.1125)),
    // EntityTypes.java:349-350
    (EntityType::DROWNED.id, zombie_like(2.0125)),
    // EntityTypes.java:363
    (EntityType::ELDER_GUARDIAN.id, b().passenger_y(2.350625)),
    // EntityTypes.java:372
    (EntityType::ENDERMAN.id, b().passenger_y(2.80625)),
    // EntityTypes.java:381
    (EntityType::ENDERMITE.id, b().passenger_y(0.2375)),
    // EntityTypes.java:387
    (EntityType::ENDER_DRAGON.id, b().passenger_y(3.0)),
    // EntityTypes.java:410-411
    (EntityType::EVOKER.id, ILLAGER),
    // EntityTypes.java:460
    (
        EntityType::FOX.id,
        b().passenger(Vector3::new(0.0, 0.6375, -0.25)),
    ),
    // EntityTypes.java:466
    (
        EntityType::FROG.id,
        b().passenger(Vector3::new(0.0, 0.375, -0.25)),
    ),
    // EntityTypes.java:470
    (EntityType::FURNACE_MINECART.id, MINECART),
    // EntityTypes.java:478-479
    (
        EntityType::GHAST.id,
        b().passenger_y(4.0625).riding_offset(0.5),
    ),
    // EntityTypes.java:488-489
    (
        EntityType::HAPPY_GHAST.id,
        b().passenger(Vector3::new(0.0, 4.0, 1.7))
            .passenger(Vector3::new(-1.7, 4.0, 0.0))
            .passenger(Vector3::new(0.0, 4.0, -1.7))
            .passenger(Vector3::new(1.7, 4.0, 0.0))
            .riding_offset(0.5),
    ),
    // EntityTypes.java:494
    (EntityType::GIANT.id, b().riding_offset(-3.75)),
    // EntityTypes.java:510
    (EntityType::GOAT.id, b().passenger_y(1.1125)),
    // EntityTypes.java:517
    (EntityType::GUARDIAN.id, b().passenger_y(0.975)),
    // EntityTypes.java:523
    (EntityType::HOGLIN.id, b().passenger_y(1.49375)),
    // EntityTypes.java:527
    (EntityType::HOPPER_MINECART.id, MINECART),
    // EntityTypes.java:531
    (EntityType::HORSE.id, b().passenger_y(1.44375)),
    // EntityTypes.java:538-539
    (EntityType::HUSK.id, zombie_like(2.075)),
    // EntityTypes.java:547-548
    (EntityType::ILLUSIONER.id, ILLAGER),
    // EntityTypes.java:620
    (
        EntityType::LLAMA.id,
        b().passenger(Vector3::new(0.0, 1.37, -0.3)),
    ),
    // EntityTypes.java:658 (`Avatar.DEFAULT_VEHICLE_ATTACHMENT`, `Avatar.java:17`)
    (
        EntityType::MANNEQUIN.id,
        b().vehicle(Vector3::new(0.0, 0.6, 0.0)),
    ),
    // EntityTypes.java:667
    (EntityType::MINECART.id, MINECART),
    // EntityTypes.java:671
    (EntityType::MOOSHROOM.id, b().passenger_y(1.36875)),
    // EntityTypes.java:675
    (EntityType::MULE.id, b().passenger_y(1.2125)),
    // EntityTypes.java:681
    (EntityType::NAUTILUS.id, b().passenger_y(1.1375)),
    // EntityTypes.java:702
    (EntityType::OCELOT.id, b().passenger_y(0.6375)),
    // EntityTypes.java:737
    (EntityType::PARCHED.id, SKELETON),
    // EntityTypes.java:741
    (EntityType::PARROT.id, b().passenger_y(0.4625)),
    // EntityTypes.java:748-749
    (
        EntityType::PHANTOM.id,
        b().passenger_y(0.3375).riding_offset(-0.125),
    ),
    // EntityTypes.java:754
    (EntityType::PIG.id, b().passenger_y(0.86875)),
    // EntityTypes.java:761-762
    (EntityType::PIGLIN.id, zombie_like(2.0125)),
    // EntityTypes.java:770-771
    (EntityType::PIGLIN_BRUTE.id, zombie_like(2.0125)),
    // EntityTypes.java:780-781
    (EntityType::PILLAGER.id, ILLAGER),
    // EntityTypes.java:815
    (
        EntityType::RAVAGER.id,
        b().passenger(Vector3::new(0.0, 2.2625, -0.0625)),
    ),
    // EntityTypes.java:824
    (EntityType::SHEEP.id, b().passenger_y(1.2375)),
    // EntityTypes.java:839
    (EntityType::SILVERFISH.id, b().passenger_y(0.2375)),
    // EntityTypes.java:845
    (EntityType::SKELETON.id, SKELETON),
    // EntityTypes.java:852
    (EntityType::SKELETON_HORSE.id, b().passenger_y(1.31875)),
    // EntityTypes.java:877
    (EntityType::SNIFFER.id, b().passenger_y(2.09375)),
    // EntityTypes.java:891
    (EntityType::SPAWNER_MINECART.id, MINECART),
    // EntityTypes.java:907
    (EntityType::SPIDER.id, b().passenger_y(0.765)),
    // EntityTypes.java:935
    (EntityType::STRAY.id, SKELETON),
    // EntityTypes.java:966
    (EntityType::TNT_MINECART.id, MINECART),
    // EntityTypes.java:973
    (
        EntityType::TRADER_LLAMA.id,
        b().passenger(Vector3::new(0.0, 1.37, -0.3)),
    ),
    // EntityTypes.java:991
    (
        EntityType::TURTLE.id,
        b().passenger(Vector3::new(0.0, 0.55625, -0.25)),
    ),
    // EntityTypes.java:999-1000
    (
        EntityType::VEX.id,
        b().passenger_y(0.7375).riding_offset(0.04),
    ),
    // EntityTypes.java:1011-1012
    (EntityType::VINDICATOR.id, ILLAGER),
    // EntityTypes.java:1024-1025
    (
        EntityType::WARDEN.id,
        b().passenger_y(3.15)
            .warden_chest(Vector3::new(0.0, 1.6f32 as f64, 0.0)),
    ),
    // EntityTypes.java:1044
    (EntityType::WITCH.id, b().passenger_y(2.2625)),
    // EntityTypes.java:1064
    (EntityType::WITHER_SKELETON.id, b().riding_offset(-0.875)),
    // EntityTypes.java:1077
    (
        EntityType::WOLF.id,
        b().passenger(Vector3::new(0.0, 0.81875, -0.0625)),
    ),
    // EntityTypes.java:1085
    (EntityType::ZOGLIN.id, b().passenger_y(1.49375)),
    // EntityTypes.java:1094-1095
    (EntityType::ZOMBIE.id, zombie_like(2.0125)),
    // EntityTypes.java:1104
    (EntityType::ZOMBIE_HORSE.id, b().passenger_y(1.31875)),
    // EntityTypes.java:1111
    (EntityType::ZOMBIE_NAUTILUS.id, b().passenger_y(1.1375)),
    // EntityTypes.java:1119-1120
    (EntityType::ZOMBIE_VILLAGER.id, zombie_like(2.125)),
    // EntityTypes.java:1131-1132
    (EntityType::ZOMBIFIED_PIGLIN.id, zombie_like(2.0)),
    // EntityTypes.java:1143 (`Avatar.DEFAULT_VEHICLE_ATTACHMENT`, `Avatar.java:17`)
    (
        EntityType::PLAYER.id,
        b().vehicle(Vector3::new(0.0, 0.6, 0.0)),
    ),
];

/// The attachment builder `EntityTypes` gives `entity_type`; fallbacks only when it has none.
#[must_use]
pub fn type_attachments(entity_type: &EntityType) -> EntityAttachmentsBuilder {
    TYPE_ATTACHMENTS
        .iter()
        .find(|(id, _)| *id == entity_type.id)
        .map_or_else(EntityAttachmentsBuilder::new, |(_, attachments)| {
            *attachments
        })
}

/// `EntityType.getDimensions()`: the type's size and eye height with its attachments applied,
/// as `EntityType.Builder.build` does (`EntityType.java:617`).
#[must_use]
pub fn type_dimensions(entity_type: &EntityType) -> EntityDimensions {
    EntityDimensions::new(
        entity_type.dimension[0],
        entity_type.dimension[1],
        entity_type.eye_height,
    )
    .with_attachments(type_attachments(entity_type))
}

#[cfg(test)]
mod tests {
    use super::type_dimensions;
    use pumpkin_data::entity::EntityType;

    /// Seat minus the rider's own vehicle point, the Y that `Entity.positionRider` adds to the
    /// vehicle position (`Entity.java:2385-2389`).
    fn seat_offset(vehicle: &EntityType, passenger: &EntityType) -> f64 {
        type_dimensions(vehicle)
            .attachments
            .passenger_clamped(0, 0.0)
            .y
            - type_dimensions(passenger).attachments.vehicle_point(0.0).y
    }

    #[test]
    fn player_seats_use_type_points() {
        assert_eq!(
            seat_offset(&EntityType::PIG, &EntityType::PLAYER),
            f64::from(0.86875f32) - 0.6
        );
        assert_eq!(
            seat_offset(&EntityType::MINECART, &EntityType::PLAYER),
            f64::from(0.1875f32) - 0.6
        );
    }

    #[test]
    fn skeleton_jockey_sits_low_on_the_spider() {
        assert_eq!(
            seat_offset(&EntityType::SPIDER, &EntityType::SKELETON),
            f64::from(0.765f32) - f64::from(0.7f32)
        );
    }

    #[test]
    fn unlisted_types_use_fallbacks() {
        let creeper = type_dimensions(&EntityType::CREEPER);
        assert_eq!(
            creeper.attachments.passenger_clamped(0, 0.0).y,
            f64::from(EntityType::CREEPER.dimension[1])
        );
        assert_eq!(creeper.attachments.vehicle_point(0.0).y, 0.0);
    }

    #[test]
    fn warden_chest_and_horse_shear_point() {
        let warden = type_dimensions(&EntityType::WARDEN);
        assert_eq!(
            warden.attachments.warden_chest_point(0.0).y,
            f64::from(1.6f32)
        );
        let horse = type_dimensions(&EntityType::HORSE);
        assert_eq!(
            horse.attachments.average_passenger().y,
            f64::from(1.44375f32)
        );
    }
}
