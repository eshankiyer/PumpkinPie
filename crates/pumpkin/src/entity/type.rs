use std::sync::Arc;

use pumpkin_data::BlockState;
use pumpkin_data::entity::{EntityType, MobCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use uuid::Uuid;

use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::boss::wither::WitherEntity;
use crate::entity::decoration::{
    armor_stand::ArmorStandEntity,
    display::{BlockDisplayEntity, ItemDisplayEntity, TextDisplayEntity},
    end_crystal::EndCrystalEntity,
    item_frame::ItemFrameEntity,
    mannequin::MannequinEntity,
    painting::PaintingEntity,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::falling::FallingEntity;
use crate::entity::interaction::InteractionEntity;
use crate::entity::item::ItemEntity;
use crate::entity::lightning::LightningBoltEntity;
use crate::entity::living::LivingEntity;
use crate::entity::marker::MarkerEntity;
use crate::entity::mob::bat::{self, BatEntity};
use crate::entity::mob::blaze::BlazeEntity;
use crate::entity::mob::breeze::BreezeEntity;
use crate::entity::mob::cave_spider::CaveSpiderEntity;
use crate::entity::mob::creaking::CreakingEntity;
use crate::entity::mob::creeper::CreeperEntity;
use crate::entity::mob::elder_guardian::ElderGuardianEntity;
use crate::entity::mob::enderman::EndermanEntity;
use crate::entity::mob::endermite::EndermiteEntity;
use crate::entity::mob::evoker::EvokerEntity;
use crate::entity::mob::ghast::GhastEntity;
use crate::entity::mob::giant::GiantEntity;
use crate::entity::mob::guardian::GuardianEntity;
use crate::entity::mob::hoglin::HoglinEntity;
use crate::entity::mob::illusioner::IllusionerEntity;
use crate::entity::mob::magma_cube::MagmaCubeEntity;
use crate::entity::mob::phantom::PhantomEntity;
use crate::entity::mob::piglin::PiglinEntity;
use crate::entity::mob::piglin_brute::PiglinBruteEntity;
use crate::entity::mob::pillager::PillagerEntity;
use crate::entity::mob::ravager::RavagerEntity;
use crate::entity::mob::shulker::ShulkerEntity;
use crate::entity::mob::silverfish::SilverfishEntity;
use crate::entity::mob::skeleton::{
    bogged::BoggedSkeletonEntity, parched::ParchedSkeletonEntity, skeleton::SkeletonEntity,
    stray::StraySkeletonEntity, wither::WitherSkeletonEntity,
};
use crate::entity::mob::slime::SlimeEntity;
use crate::entity::mob::spider::SpiderEntity;
use crate::entity::mob::sulfur_cube::SulfurCubeEntity;
use crate::entity::mob::vex::VexEntity;
use crate::entity::mob::vindicator::VindicatorEntity;
use crate::entity::mob::warden::WardenEntity;
use crate::entity::mob::witch::WitchEntity;
use crate::entity::mob::zoglin::ZoglinEntity;
use crate::entity::mob::zombie::zombie_villager::ZombieVillagerEntity;
use crate::entity::mob::zombie::{drowned::DrownedEntity, husk::HuskEntity, zombie::ZombieEntity};
use crate::entity::mob::zombified_piglin::ZombifiedPiglinEntity;
use crate::entity::passive::allay::AllayEntity;
use crate::entity::passive::armadillo::ArmadilloEntity;
use crate::entity::passive::axolotl::AxolotlEntity;
use crate::entity::passive::bee::BeeEntity;
use crate::entity::passive::camel::CamelEntity;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::chicken::ChickenEntity;
use crate::entity::passive::cod::CodEntity;
use crate::entity::passive::copper_golem::CopperGolemEntity;
use crate::entity::passive::cow::CowEntity;
use crate::entity::passive::dolphin::DolphinEntity;
use crate::entity::passive::donkey::DonkeyEntity;
use crate::entity::passive::fox::FoxEntity;
use crate::entity::passive::frog::FrogEntity;
use crate::entity::passive::glow_squid::GlowSquidEntity;
use crate::entity::passive::goat::GoatEntity;
use crate::entity::passive::happy_ghast::HappyGhastEntity;
use crate::entity::passive::horse::HorseEntity;
use crate::entity::passive::iron_golem::IronGolemEntity;
use crate::entity::passive::llama::LlamaEntity;
use crate::entity::passive::mooshroom::MooshroomEntity;
use crate::entity::passive::mule::MuleEntity;
use crate::entity::passive::nautilus::NautilusEntity;
use crate::entity::passive::ocelot::OcelotEntity;
use crate::entity::passive::panda::PandaEntity;
use crate::entity::passive::parrot::ParrotEntity;
use crate::entity::passive::pig::PigEntity;
use crate::entity::passive::polar_bear::PolarBearEntity;
use crate::entity::passive::pufferfish::PufferfishEntity;
use crate::entity::passive::rabbit::RabbitEntity;
use crate::entity::passive::salmon::SalmonEntity;
use crate::entity::passive::sheep::SheepEntity;
use crate::entity::passive::skeleton_horse::SkeletonHorseEntity;
use crate::entity::passive::sniffer::SnifferEntity;
use crate::entity::passive::snow_golem::SnowGolemEntity;
use crate::entity::passive::squid::SquidEntity;
use crate::entity::passive::strider::StriderEntity;
use crate::entity::passive::tadpole::TadpoleEntity;
use crate::entity::passive::trader_llama::TraderLlamaEntity;
use crate::entity::passive::tropical_fish::TropicalFishEntity;
use crate::entity::passive::turtle::TurtleEntity;
use crate::entity::passive::villager::VillagerEntity;
use crate::entity::passive::wandering_trader::WanderingTraderEntity;
use crate::entity::passive::wolf::WolfEntity;
use crate::entity::passive::zombie_horse::ZombieHorseEntity;
use crate::entity::projectile::ThrownItemEntity;
use crate::entity::projectile::arrow::ArrowEntity;
use crate::entity::projectile::egg::EggEntity;
use crate::entity::projectile::ender_pearl::EnderPearlEntity;
use crate::entity::projectile::evoker_fangs::EvokerFangsEntity;
use crate::entity::projectile::experience_bottle::ExperienceBottleEntity;
use crate::entity::projectile::eye_of_ender::EyeOfEnder;
use crate::entity::projectile::fireball::FireballEntity;
use crate::entity::projectile::firework_rocket::FireworkRocketEntity;
use crate::entity::projectile::lingering_potion::LingeringPotionEntity;
use crate::entity::projectile::llama_spit::LlamaSpitEntity;
use crate::entity::projectile::shulker_bullet::ShulkerBulletEntity;
use crate::entity::projectile::small_fireball::SmallFireballEntity;
use crate::entity::projectile::snowball::SnowballEntity;
use crate::entity::projectile::splash_potion::SplashPotionEntity;
use crate::entity::projectile::trident::TridentEntity;
use crate::entity::projectile::wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity};
use crate::entity::projectile::wither_skull::WitherSkullEntity;
use crate::entity::tnt::TNTEntity;
use crate::entity::vehicle::boat::BoatEntity;
use crate::entity::vehicle::minecart::MinecartEntity;
use crate::entity::{Entity, EntityBase, mob};
use crate::world::World;
use pumpkin_data::Block;
use std::sync::atomic::AtomicBool;

#[expect(clippy::too_many_lines)]
pub fn from_type(
    entity_type: &'static EntityType,
    position: Vector3<f64>,
    world: &Arc<World>,
    uuid: Uuid,
) -> Arc<dyn EntityBase> {
    let entity = Entity::from_uuid(uuid, world.clone(), position, entity_type);

    let mob: Arc<dyn EntityBase> = match entity_type.id {
        // Zombie
        id if id == EntityType::ZOMBIE.id => ZombieEntity::new(entity),
        id if id == EntityType::DROWNED.id => DrownedEntity::new(entity),
        id if id == EntityType::HUSK.id => HuskEntity::new(entity),
        id if id == EntityType::ZOMBIE_VILLAGER.id => ZombieVillagerEntity::new(entity),
        id if id == EntityType::ZOMBIFIED_PIGLIN.id => ZombifiedPiglinEntity::new(entity),

        // Skeleton
        id if id == EntityType::SKELETON.id => SkeletonEntity::new(entity),
        id if id == EntityType::BOGGED.id => BoggedSkeletonEntity::new(entity),
        id if id == EntityType::PARCHED.id => ParchedSkeletonEntity::new(entity),
        id if id == EntityType::WITHER_SKELETON.id => WitherSkeletonEntity::new(entity),
        id if id == EntityType::STRAY.id => StraySkeletonEntity::new(entity),

        id if id == EntityType::BAT.id => BatEntity::new(entity),
        id if id == EntityType::CREEPER.id => CreeperEntity::new(entity),
        id if id == EntityType::ENDERMAN.id => EndermanEntity::new(entity),

        id if id == EntityType::BLAZE.id => BlazeEntity::new(entity),
        id if id == EntityType::SPIDER.id => SpiderEntity::new(entity),
        id if id == EntityType::CAVE_SPIDER.id => CaveSpiderEntity::new(entity),
        id if id == EntityType::GHAST.id => GhastEntity::new(entity),
        id if id == EntityType::MAGMA_CUBE.id => MagmaCubeEntity::new(entity),
        id if id == EntityType::PHANTOM.id => PhantomEntity::new(entity),
        id if id == EntityType::WITCH.id => WitchEntity::new(entity),
        id if id == EntityType::PIGLIN.id => PiglinEntity::new(entity),
        id if id == EntityType::PIGLIN_BRUTE.id => PiglinBruteEntity::new(entity),
        id if id == EntityType::PILLAGER.id => PillagerEntity::new(entity),
        id if id == EntityType::VINDICATOR.id => VindicatorEntity::new(entity),
        id if id == EntityType::EVOKER.id => EvokerEntity::new(entity),
        id if id == EntityType::RAVAGER.id => RavagerEntity::new(entity),
        id if id == EntityType::GUARDIAN.id => GuardianEntity::new(entity),
        id if id == EntityType::ELDER_GUARDIAN.id => ElderGuardianEntity::new(entity),
        id if id == EntityType::WARDEN.id => WardenEntity::new(entity),
        id if id == EntityType::HOGLIN.id => HoglinEntity::new(entity),
        id if id == EntityType::ZOGLIN.id => ZoglinEntity::new(entity),
        id if id == EntityType::BREEZE.id => BreezeEntity::new(entity),
        id if id == EntityType::CREAKING.id => CreakingEntity::new(entity),
        id if id == EntityType::ILLUSIONER.id => IllusionerEntity::new(entity),
        id if id == EntityType::VEX.id => VexEntity::new(entity),
        id if id == EntityType::ENDERMITE.id => EndermiteEntity::new(entity),
        id if id == EntityType::GIANT.id => GiantEntity::new(entity),

        id if id == EntityType::CAT.id => CatEntity::new(entity),
        id if id == EntityType::CHICKEN.id => ChickenEntity::new(entity),
        id if id == EntityType::COW.id => CowEntity::new(entity),
        id if id == EntityType::PIG.id => PigEntity::new(entity),
        id if id == EntityType::SHEEP.id => SheepEntity::new(entity),
        id if id == EntityType::WOLF.id => WolfEntity::new(entity),
        id if id == EntityType::FOX.id => FoxEntity::new(entity),
        id if id == EntityType::RABBIT.id => RabbitEntity::new(entity),
        id if id == EntityType::TURTLE.id => TurtleEntity::new(entity),
        id if id == EntityType::VILLAGER.id => VillagerEntity::new(entity),
        id if id == EntityType::SQUID.id => SquidEntity::new(entity),
        id if id == EntityType::HORSE.id => HorseEntity::new(entity),
        id if id == EntityType::DONKEY.id => DonkeyEntity::new(entity),
        id if id == EntityType::MULE.id => MuleEntity::new(entity),
        id if id == EntityType::ZOMBIE_HORSE.id => ZombieHorseEntity::new(entity),
        id if id == EntityType::SKELETON_HORSE.id => SkeletonHorseEntity::new(entity),
        id if id == EntityType::LLAMA.id => LlamaEntity::new(entity),
        id if id == EntityType::TRADER_LLAMA.id => TraderLlamaEntity::new(entity),
        id if id == EntityType::WANDERING_TRADER.id => WanderingTraderEntity::new(entity),
        id if id == EntityType::ALLAY.id => AllayEntity::new(entity),
        id if id == EntityType::ARMADILLO.id => ArmadilloEntity::new(entity),
        id if id == EntityType::AXOLOTL.id => AxolotlEntity::new(entity),
        id if id == EntityType::BEE.id => BeeEntity::new(entity),
        id if id == EntityType::CAMEL.id => CamelEntity::new(entity),
        id if id == EntityType::COPPER_GOLEM.id => CopperGolemEntity::new(entity),
        id if id == EntityType::FROG.id => FrogEntity::new(entity),
        id if id == EntityType::GOAT.id => GoatEntity::new(entity),
        id if id == EntityType::HAPPY_GHAST.id => HappyGhastEntity::new(entity),
        id if id == EntityType::MOOSHROOM.id => MooshroomEntity::new(entity),
        id if id == EntityType::OCELOT.id => OcelotEntity::new(entity),
        id if id == EntityType::NAUTILUS.id => NautilusEntity::new(entity),
        id if id == EntityType::PANDA.id => PandaEntity::new(entity),
        id if id == EntityType::PARROT.id => ParrotEntity::new(entity),
        id if id == EntityType::POLAR_BEAR.id => PolarBearEntity::new(entity),
        id if id == EntityType::SNIFFER.id => SnifferEntity::new(entity),
        id if id == EntityType::STRIDER.id => StriderEntity::new(entity),
        id if id == EntityType::GLOW_SQUID.id => GlowSquidEntity::new(entity),
        id if id == EntityType::COD.id => CodEntity::new(entity),
        id if id == EntityType::SALMON.id => SalmonEntity::new(entity),
        id if id == EntityType::PUFFERFISH.id => PufferfishEntity::new(entity),
        id if id == EntityType::TROPICAL_FISH.id => TropicalFishEntity::new(entity),
        id if id == EntityType::TADPOLE.id => TadpoleEntity::new(entity),
        id if id == EntityType::DOLPHIN.id => DolphinEntity::new(entity),

        id if id == EntityType::SNOW_GOLEM.id => SnowGolemEntity::new(entity),
        id if id == EntityType::IRON_GOLEM.id => IronGolemEntity::new(entity),

        id if id == EntityType::WITHER.id => WitherEntity::new(entity),
        id if id == EntityType::ENDER_DRAGON.id => EnderDragonEntity::new(entity),

        id if id == EntityType::AREA_EFFECT_CLOUD.id => {
            crate::entity::area_effect_cloud::AreaEffectCloudEntity::new(entity)
        }
        id if id == EntityType::ARMOR_STAND.id => Arc::new(ArmorStandEntity::new(entity)),
        id if id == EntityType::PAINTING.id => Arc::new(PaintingEntity::new(entity)),
        id if id == EntityType::ITEM_FRAME.id || id == EntityType::GLOW_ITEM_FRAME.id => {
            Arc::new(ItemFrameEntity::new(entity))
        }
        id if id == EntityType::END_CRYSTAL.id => Arc::new(EndCrystalEntity::new(entity)),
        id if id == EntityType::MANNEQUIN.id => Arc::new(MannequinEntity::new(entity)),
        id if id == EntityType::TEXT_DISPLAY.id => TextDisplayEntity::new(entity),
        id if id == EntityType::ITEM_DISPLAY.id => ItemDisplayEntity::new(entity),
        id if id == EntityType::BLOCK_DISPLAY.id => BlockDisplayEntity::new(entity),
        id if id == EntityType::ENDER_PEARL.id => Arc::new(EnderPearlEntity::new(entity)),
        id if id == EntityType::SNOWBALL.id => Arc::new(SnowballEntity::new(entity)),
        id if id == EntityType::EGG.id => Arc::new(EggEntity::new(entity)),
        id if id == EntityType::EXPERIENCE_BOTTLE.id => {
            Arc::new(ExperienceBottleEntity::new(entity))
        }
        id if id == EntityType::SILVERFISH.id => SilverfishEntity::new(entity),
        id if id == EntityType::SLIME.id => SlimeEntity::new(entity),
        id if id == EntityType::SULFUR_CUBE.id => SulfurCubeEntity::new(entity),
        id if id == EntityType::SHULKER.id => ShulkerEntity::new(entity),
        id if id == EntityType::SHULKER_BULLET.id => {
            // Shulker bullets are normally spawned by ShulkerEntity directly;
            // when loaded from the world we create a no-target bullet at the given position.
            Arc::new(ShulkerBulletEntity::orphan(entity))
        }
        id if id == EntityType::EVOKER_FANGS.id => {
            // Normally spawned directly by EvokerAttackSpellGoal; loaded/summoned fangs have no
            // casting owner.
            Arc::new(EvokerFangsEntity::orphan(entity))
        }
        id if id == EntityType::FALLING_BLOCK.id => {
            Arc::new(FallingEntity::new(entity, Block::SAND.default_state.id))
        }
        id if id == EntityType::EXPERIENCE_ORB.id => Arc::new(ExperienceOrbEntity::new(entity, 1)),
        id if id == EntityType::TNT.id => Arc::new(TNTEntity::new(entity, 4.0, 80)),
        id if id == EntityType::ITEM.id => Arc::new(ItemEntity::new_for_restore(entity)),
        id if id == EntityType::ARROW.id => Arc::new(ArrowEntity::new(entity, None)),
        id if id == EntityType::SPECTRAL_ARROW.id => Arc::new(ArrowEntity::new(entity, None)),
        id if id == EntityType::TRIDENT.id => Arc::new(TridentEntity::new(entity, None)),
        id if id == EntityType::MINECART.id
            || id == EntityType::CHEST_MINECART.id
            || id == EntityType::FURNACE_MINECART.id
            || id == EntityType::TNT_MINECART.id
            || id == EntityType::HOPPER_MINECART.id
            || id == EntityType::COMMAND_BLOCK_MINECART.id
            || id == EntityType::SPAWNER_MINECART.id =>
        {
            Arc::new(MinecartEntity::new(entity))
        }
        id if id == EntityType::FIREBALL.id => Arc::new(FireballEntity::new(entity)),
        id if id == EntityType::SMALL_FIREBALL.id => Arc::new(SmallFireballEntity::new(entity)),
        id if id == EntityType::WITHER_SKULL.id => Arc::new(WitherSkullEntity::new(entity)),
        id if id == EntityType::LLAMA_SPIT.id => Arc::new(LlamaSpitEntity::new(entity)),
        id if id == EntityType::WIND_CHARGE.id => {
            let thrown = ThrownItemEntity {
                entity,
                owner_id: None,
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: WIND_CHARGE_GRAVITY,
            };
            Arc::new(WindChargeEntity::new_normal(thrown))
        }
        id if id == EntityType::BREEZE_WIND_CHARGE.id => {
            let thrown = ThrownItemEntity {
                entity,
                owner_id: None,
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: WIND_CHARGE_GRAVITY,
            };
            Arc::new(WindChargeEntity::new_breeze(thrown))
        }
        id if id == EntityType::LIGHTNING_BOLT.id => Arc::new(LightningBoltEntity::new(entity)),
        id if id == EntityType::MARKER.id => MarkerEntity::new(entity),
        id if id == EntityType::INTERACTION.id => InteractionEntity::new(entity),
        id if id == EntityType::FIREWORK_ROCKET.id => Arc::new(FireworkRocketEntity::new(entity)),
        id if id == EntityType::SPLASH_POTION.id => Arc::new(SplashPotionEntity::new(entity)),
        id if id == EntityType::LINGERING_POTION.id => Arc::new(LingeringPotionEntity::new(entity)),
        id if id == EntityType::EYE_OF_ENDER.id => Arc::new(EyeOfEnder::new(entity)),
        id if id == EntityType::ACACIA_BOAT.id
            || id == EntityType::ACACIA_CHEST_BOAT.id
            || id == EntityType::BIRCH_BOAT.id
            || id == EntityType::BIRCH_CHEST_BOAT.id
            || id == EntityType::DARK_OAK_BOAT.id
            || id == EntityType::DARK_OAK_CHEST_BOAT.id
            || id == EntityType::JUNGLE_BOAT.id
            || id == EntityType::JUNGLE_CHEST_BOAT.id
            || id == EntityType::MANGROVE_BOAT.id
            || id == EntityType::MANGROVE_CHEST_BOAT.id
            || id == EntityType::OAK_BOAT.id
            || id == EntityType::OAK_CHEST_BOAT.id
            || id == EntityType::PALE_OAK_BOAT.id
            || id == EntityType::PALE_OAK_CHEST_BOAT.id
            || id == EntityType::SPRUCE_BOAT.id
            || id == EntityType::SPRUCE_CHEST_BOAT.id
            || id == EntityType::BAMBOO_RAFT.id
            || id == EntityType::BAMBOO_CHEST_RAFT.id
            || id == EntityType::CHERRY_BOAT.id
            || id == EntityType::CHERRY_CHEST_BOAT.id =>
        {
            Arc::new(BoatEntity::new(entity))
        }
        // Fallback Entity
        _ => {
            if entity_type.attributes.is_empty() {
                Arc::new(entity)
            } else {
                Arc::new(LivingEntity::new(entity))
            }
        }
    };

    mob
}

/// The two non-spawner contexts that reach `check_spawn_rules`.
///
/// Vanilla evaluates generation-time predicates against a `WorldGenRegion`, whose
/// `getNearestPlayer` is overridden to return `null` and whose `players()` returns an
/// empty list (`WorldGenRegion.java:173-175`, `482-484`), while natural spawning
/// evaluates the same predicates against the live server world. Pumpkin runs both
/// paths against the same `World`, so the caller has to say which one it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnRuleContext {
    ChunkGeneration,
    Natural,
}

#[allow(clippy::too_many_lines)]
pub fn check_spawn_rules(
    entity_type: &'static EntityType,
    world: &World,
    pos: &BlockPos,
    context: SpawnRuleContext,
    is_thundering: bool,
) -> bool {
    let id = entity_type.id;

    if id == EntityType::GUARDIAN.id || id == EntityType::ELDER_GUARDIAN.id {
        if world.level_info.load().difficulty == pumpkin_util::Difficulty::Peaceful {
            return false;
        }
        if !world.get_fluid(pos).has_tag(&tag::Fluid::MINECRAFT_WATER)
            || !world
                .get_fluid(&pos.down())
                .has_tag(&tag::Fluid::MINECRAFT_WATER)
        {
            return false;
        }

        return rand::random_range(0u8..20) == 0 || !world.can_see_sky_from_below_water(pos);
    }

    // `Monster.checkSurfaceMonstersSpawnRules` (Monster.java:113-117) adds sky visibility
    // to the usual monster darkness and placement checks. Its `EntitySpawnReason.isSpawner`
    // escape covers only SPAWNER and TRIAL_SPAWNER (EntitySpawnReason.java:23-25), neither of
    // which reaches this function, so the sky check always applies here.
    if uses_surface_monster_spawn_rules(id) {
        return surface_monster_spawn_rules_allowed(
            mob::MobEntity::check_monster_spawn_rules(world, pos, is_thundering),
            world.can_see_sky(pos),
        );
    }

    if id == EntityType::MAGMA_CUBE.id {
        return world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful;
    }

    // `Ghast.checkGhastSpawnRules` (Ghast.java:149-153): no darkness requirement, but only
    // one in twenty attempts succeeds. Its `checkMobSpawnRules` call is the support-block
    // test, which the ON_GROUND placement check already applies before this function runs.
    if id == EntityType::GHAST.id {
        return world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful
            && ghast_spawn_roll_allowed(rand::random_range(0u8..20));
    }

    // `Stray.checkStraySpawnRules` (Stray.java:26-37) requires ordinary monster spawning plus
    // visible sky. Powder snow above the candidate does not block that sky check, so walk
    // through it before testing.
    if id == EntityType::STRAY.id {
        return check_stray_spawn_rules(world, pos, is_thundering);
    }

    if id == EntityType::DROWNED.id {
        if !world
            .get_fluid(&pos.down())
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
        {
            return false;
        }

        let can_spawn = world.get_fluid(pos).has_tag(&tag::Fluid::MINECRAFT_WATER)
            && mob::MobEntity::check_monster_spawn_rules(world, pos, is_thundering);
        if !can_spawn {
            return false;
        }

        // Positive membership test over a fixed tag: an unresolvable biome is not in it, so
        // fall through to the ordinary (rarer) drowned chance rather than inventing a biome.
        // This only selects between two spawn probabilities - the spawn is already gated by
        // the water/monster checks above - so it cannot create a spawn vanilla would refuse.
        if world.get_biome(pos).is_some_and(|biome| {
            biome.has_tag(&tag::WorldgenBiome::MINECRAFT_MORE_FREQUENT_DROWNED_SPAWNS)
        }) {
            return rand::random_range(0u8..15) == 0;
        }

        return pos.0.y < world.sea_level - 5 && rand::random_range(0u8..40) == 0;
    }

    // `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules`: only in a shallow
    // band just below sea level, directly under a water surface (block above is water).
    if id == EntityType::COD.id
        || id == EntityType::DOLPHIN.id
        || id == EntityType::PUFFERFISH.id
        || id == EntityType::SALMON.id
        || id == EntityType::SQUID.id
    {
        return check_surface_water_creature_spawn_rules(world, pos);
    }

    // `TropicalFish.checkTropicalFishSpawnRules`: tropical fish can use the same surface
    // placement, or any height in biomes carrying the special vanilla biome tag.
    if id == EntityType::TROPICAL_FISH.id {
        let water_placement = world
            .get_fluid(&pos.down())
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
            && world.get_block(&pos.up()) == &Block::WATER;
        return water_placement
            && (world.get_biome(pos).is_some_and(|biome| {
                biome.has_tag(
                    &tag::WorldgenBiome::MINECRAFT_ALLOWS_TROPICAL_FISH_SPAWNS_AT_ANY_HEIGHT,
                )
            }) || is_in_surface_water_y_range(pos.0.y, world.sea_level));
    }

    // `AbstractNautilus.checkNautilusSpawnRules`: five blocks below the surface through
    // twenty-five blocks below it, with water immediately below and above the spawn position.
    if id == EntityType::NAUTILUS.id {
        return is_in_nautilus_y_range(pos.0.y, world.sea_level)
            && world
                .get_fluid(&pos.down())
                .has_tag(&tag::Fluid::MINECRAFT_WATER)
            && world.get_block(&pos.up()) == &Block::WATER;
    }

    // `Axolotl.checkAxolotlSpawnRules` only checks the block tag below the candidate.
    if id == EntityType::AXOLOTL.id {
        return world
            .get_block(&pos.down())
            .has_tag(&tag::Block::MINECRAFT_AXOLOTLS_SPAWNABLE_ON);
    }

    // `Armadillo.checkArmadilloSpawnRules` and `Camel.checkCamelSpawnRules` both
    // require their species-specific ground tag and daylight, rather than the
    // generic animal tag used by cattle, sheep, and similar mobs.
    if id == EntityType::ARMADILLO.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_ARMADILLO_SPAWNABLE_ON,
        );
    }
    if id == EntityType::CAMEL.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_CAMELS_SPAWNABLE_ON,
        );
    }
    if id == EntityType::WOLF.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_WOLVES_SPAWNABLE_ON,
        );
    }

    // `GlowSquid.checkGlowSquidSpawnRules`: only deep below sea level, in complete darkness.
    if id == EntityType::GLOW_SQUID.id {
        return is_below_glow_squid_y_threshold(pos.0.y, world.sea_level)
            && world.get_raw_brightness(pos, 0) == 0
            && world.get_block(pos) == &Block::WATER;
    }

    // `Ocelot.checkOcelotSpawnRules`: natural attempts succeed on two of
    // three calls. Its obstruction predicate is applied by the natural
    // spawner after this common predicate.
    if id == EntityType::OCELOT.id {
        return ocelot_spawn_roll_allowed(rand::random_range(0u8..3));
    }

    // `Fox.checkFoxSpawnRules`: foxes use the dedicated ground tag and daylight,
    // even though their placement type is `NO_RESTRICTIONS`.
    if id == EntityType::FOX.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_FOXES_SPAWNABLE_ON,
        );
    }

    // `Hoglin.checkHoglinSpawnRules`: the block below must not be a Nether Wart Block.
    if id == EntityType::HOGLIN.id {
        return world.get_block(&pos.down()) != &Block::NETHER_WART_BLOCK;
    }

    // `Piglin.checkPiglinSpawnRules`: piglins cannot spawn on Nether Wart Blocks.
    if id == EntityType::PIGLIN.id {
        return world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful
            && world.get_block(&pos.down()) != &Block::NETHER_WART_BLOCK;
    }

    // `ZombifiedPiglin.checkZombifiedPiglinSpawnRules` (ZombifiedPiglin.java:170-174) uses
    // the same peaceful and Nether Wart Block gates as piglins, but has no darkness or
    // support-block requirement.
    if id == EntityType::ZOMBIFIED_PIGLIN.id {
        return zombified_piglin_spawn_rules_allowed(
            world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful,
            world.get_block(&pos.down()) != &Block::NETHER_WART_BLOCK,
        );
    }

    // `Endermite.checkEndermiteSpawnRules` (Endermite.java:131-144) and
    // `Silverfish.checkSilverfishSpawnRules` (Silverfish.java:111-124) use the any-light
    // predicate, then reject the attempt when `getNearestPlayer(.., 5.0, true)` finds a
    // player - `true` selects `EntitySelector.NO_CREATIVE_OR_SPECTATOR`
    // (EntityGetter.java:95-97, EntitySelector.java:14-16). A `WorldGenRegion` has no
    // players at all, so the query cannot fail on the generation path.
    if uses_nearby_player_restricted_any_light_spawn_rules(id) {
        let has_nearby_non_creative_player =
            !no_non_creative_player_within_spawn_exclusion_range(world, pos);
        return nearby_player_restricted_any_light_spawn_rules_allowed(
            context,
            mob::MobEntity::check_any_light_monster_spawn_rules(world, pos),
            has_nearby_non_creative_player,
        );
    }

    // `Strider.checkStriderSpawnRules`: after the lava placement check, walk
    // upward through lava and require the first non-lava block to be air.
    if id == EntityType::STRIDER.id {
        return check_strider_spawn_rules(world, pos);
    }

    // `Goat.checkGoatSpawnRules`: goats require their dedicated ground tag and daylight.
    if id == EntityType::GOAT.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_GOATS_SPAWNABLE_ON,
        );
    }

    // `Frog.checkFrogSpawnRules`: frogs require their dedicated ground tag and daylight.
    if id == EntityType::FROG.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_FROGS_SPAWNABLE_ON,
        );
    }

    // `Rabbit.checkRabbitSpawnRules`: rabbits require their dedicated ground tag and daylight.
    if id == EntityType::RABBIT.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_RABBITS_SPAWNABLE_ON,
        );
    }

    // `MushroomCow.checkMushroomSpawnRules`: mooshrooms require mycelium and daylight.
    if id == EntityType::MOOSHROOM.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_MOOSHROOMS_SPAWNABLE_ON,
        );
    }

    // `Parrot.checkParrotSpawnRules`: parrots require their dedicated ground tag and daylight.
    if id == EntityType::PARROT.id {
        return check_bright_ground_spawn_rules(
            world,
            pos,
            &tag::Block::MINECRAFT_PARROTS_SPAWNABLE_ON,
        );
    }

    // `Turtle.checkTurtleSpawnRules`: turtles spawn below sea level + 4 on sand in daylight.
    if id == EntityType::TURTLE.id {
        return pos.0.y < world.sea_level + 4
            && check_bright_ground_spawn_rules(world, pos, &tag::Block::MINECRAFT_SAND);
    }

    // `PolarBear.checkPolarBearSpawnRules`: alternate biomes use their own ground tag;
    // all other biomes use the ordinary animal predicate.
    if id == EntityType::POLAR_BEAR.id {
        let Some(biome) = world.get_biome(pos) else {
            return false;
        };
        if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_POLAR_BEARS_SPAWN_ON_ALTERNATE_BLOCKS) {
            return check_bright_ground_spawn_rules(
                world,
                pos,
                &tag::Block::MINECRAFT_POLAR_BEARS_SPAWNABLE_ON_ALTERNATE,
            );
        }

        return world
            .get_block(&pos.down())
            .has_tag(&tag::Block::MINECRAFT_ANIMALS_SPAWNABLE_ON)
            && world.get_raw_brightness(pos, 0) > 8;
    }

    if uses_animal_spawn_rules(id) {
        return world
            .get_block(&pos.down())
            .has_tag(&tag::Block::MINECRAFT_ANIMALS_SPAWNABLE_ON)
            && world.get_raw_brightness(pos, 0) > 8;
    }

    if entity_type.category == &MobCategory::MONSTER && uses_any_light_monster_spawn_rules(id) {
        return mob::MobEntity::check_any_light_monster_spawn_rules(world, pos);
    }

    if entity_type.category == &MobCategory::MONSTER && uses_generic_monster_spawn_rules(id) {
        return mob::MobEntity::check_monster_spawn_rules(world, pos, is_thundering);
    }

    if id == EntityType::BOGGED.id
        || id == EntityType::CAVE_SPIDER.id
        || id == EntityType::CREEPER.id
        || id == EntityType::ENDERMAN.id
        || id == EntityType::GIANT.id
        || id == EntityType::RAVAGER.id
        || id == EntityType::SKELETON.id
        || id == EntityType::SPIDER.id
        || id == EntityType::WITCH.id
        || id == EntityType::WITHER.id
        || id == EntityType::WITHER_SKELETON.id
        || id == EntityType::ZOMBIE.id
        || id == EntityType::ZOMBIE_HORSE.id
        || id == EntityType::ZOMBIE_VILLAGER.id
        || id == EntityType::CREAKING.id
        || id == EntityType::EVOKER.id
        || id == EntityType::ILLUSIONER.id
        || id == EntityType::VEX.id
        || id == EntityType::VINDICATOR.id
        || id == EntityType::WARDEN.id
    {
        return mob::MobEntity::check_monster_spawn_rules(world, pos, is_thundering);
    }
    if id == EntityType::BAT.id {
        return bat::BatEntity::check_bat_spawn_rules(world, pos);
    }
    if id == EntityType::SLIME.id {
        return SlimeEntity::check_slime_spawn_rules(world, pos);
    }

    // TODO
    true
}

/// Entity types registered with `Animal.checkAnimalSpawnRules` in vanilla's `SpawnPlacements`.
const fn uses_animal_spawn_rules(id: u16) -> bool {
    id == EntityType::CAT.id
        || id == EntityType::CHICKEN.id
        || id == EntityType::COW.id
        || id == EntityType::DONKEY.id
        || id == EntityType::HAPPY_GHAST.id
        || id == EntityType::HORSE.id
        || id == EntityType::LLAMA.id
        || id == EntityType::MULE.id
        || id == EntityType::PANDA.id
        || id == EntityType::PIG.id
        || id == EntityType::SHEEP.id
        || id == EntityType::SKELETON_HORSE.id
        || id == EntityType::TRADER_LLAMA.id
}

/// `MobCategory::MONSTER` members whose registered `SpawnPlacements` predicate is not
/// `Monster.checkMonsterSpawnRules`. Slime and hoglin each have a dedicated predicate,
/// so the category-wide branch must not answer for them.
const fn uses_generic_monster_spawn_rules(id: u16) -> bool {
    id != EntityType::SLIME.id
        && id != EntityType::HOGLIN.id
        && !uses_any_light_monster_spawn_rules(id)
}

/// Monster types registered with `Monster.checkAnyLightMonsterSpawnRules` in
/// vanilla's `SpawnPlacements`.
const fn uses_any_light_monster_spawn_rules(id: u16) -> bool {
    id == EntityType::BLAZE.id || id == EntityType::BREEZE.id || id == EntityType::ZOGLIN.id
}

/// `Endermite` and `Silverfish` use the any-light predicate plus a nearby-player exclusion.
const fn uses_nearby_player_restricted_any_light_spawn_rules(id: u16) -> bool {
    id == EntityType::ENDERMITE.id || id == EntityType::SILVERFISH.id
}

fn nearby_player_restricted_any_light_spawn_rules_allowed(
    context: SpawnRuleContext,
    any_light_rules_pass: bool,
    has_nearby_non_creative_player: bool,
) -> bool {
    any_light_rules_pass
        && (context == SpawnRuleContext::ChunkGeneration || !has_nearby_non_creative_player)
}

/// `SpawnPlacements` registrations using `Monster.checkSurfaceMonstersSpawnRules`
/// (SpawnPlacements.java:113, 129, 157).
const fn uses_surface_monster_spawn_rules(id: u16) -> bool {
    id == EntityType::CAMEL_HUSK.id || id == EntityType::HUSK.id || id == EntityType::PARCHED.id
}

/// `Monster.checkSurfaceMonstersSpawnRules` outside a spawner.
const fn surface_monster_spawn_rules_allowed(monster_rules_pass: bool, can_see_sky: bool) -> bool {
    monster_rules_pass && can_see_sky
}

/// `Ghast.checkGhastSpawnRules`'s one-in-twenty roll after the difficulty gate.
const fn ghast_spawn_roll_allowed(roll: u8) -> bool {
    roll == 0
}

/// `ZombifiedPiglin.checkZombifiedPiglinSpawnRules`.
const fn zombified_piglin_spawn_rules_allowed(
    is_non_peaceful: bool,
    below_is_not_nether_wart: bool,
) -> bool {
    is_non_peaceful && below_is_not_nether_wart
}

fn no_non_creative_player_within_spawn_exclusion_range(world: &World, pos: &BlockPos) -> bool {
    world
        .get_closest_player_where(pos.to_centered_f64(), 5.0, |player| {
            !matches!(
                player.gamemode.load(),
                pumpkin_util::GameMode::Creative | pumpkin_util::GameMode::Spectator
            )
        })
        .is_none()
}

/// `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules`'s Y-range gate:
/// `pos.getY() >= seaLevel - 13 && pos.getY() <= seaLevel`.
const fn is_in_surface_water_y_range(y: i32, sea_level: i32) -> bool {
    y >= sea_level - 13 && y <= sea_level
}

fn check_surface_water_creature_spawn_rules(world: &World, pos: &BlockPos) -> bool {
    is_in_surface_water_y_range(pos.0.y, world.sea_level)
        && world
            .get_fluid(&pos.down())
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
        && world.get_block(&pos.up()) == &Block::WATER
}

fn check_strider_spawn_rules(world: &World, pos: &BlockPos) -> bool {
    let mut check_pos = pos.up();
    while world
        .get_fluid(&check_pos)
        .has_tag(&tag::Fluid::MINECRAFT_LAVA)
    {
        check_pos = check_pos.up();
    }

    world.get_block_state(&check_pos).is_air()
}

fn check_stray_spawn_rules(world: &World, pos: &BlockPos, is_thundering: bool) -> bool {
    let mut check_sky_pos = pos.up();
    while world.get_block(&check_sky_pos) == &Block::POWDER_SNOW {
        check_sky_pos = check_sky_pos.up();
    }

    mob::MobEntity::check_monster_spawn_rules(world, pos, is_thundering)
        && world.can_see_sky(&check_sky_pos.down())
}

fn check_bright_ground_spawn_rules(
    world: &World,
    pos: &BlockPos,
    spawnable_on: &'static tag::Tag,
) -> bool {
    world.get_block(&pos.down()).has_tag(spawnable_on) && world.get_raw_brightness(pos, 0) > 8
}

/// `Ocelot.checkSpawnObstruction`'s species-specific condition.
///
/// The common spawn-position checks already cover collision and fluids.
pub fn check_spawn_obstruction(
    world: &World,
    pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    if entity_type.id != EntityType::OCELOT.id {
        return true;
    }

    check_spawn_obstruction_state(
        pos.0.y,
        world.sea_level,
        world.get_block_state(&pos.down()),
        ocelot_contains_any_liquid(world, pos, entity_type),
        ocelot_has_entity_collision(world, pos, entity_type),
        entity_type,
    )
}

#[must_use]
pub fn check_spawn_obstruction_state(
    y: i32,
    sea_level: i32,
    below: &'static BlockState,
    contains_any_liquid: bool,
    has_entity_collision: bool,
    entity_type: &'static EntityType,
) -> bool {
    if entity_type.id != EntityType::OCELOT.id {
        return true;
    }

    let below_block = Block::from_state_id(below.id);
    ocelot_spawn_obstruction_allowed(
        y,
        sea_level,
        below_block == &Block::GRASS_BLOCK,
        below_block.has_tag(&tag::Block::MINECRAFT_LEAVES),
        contains_any_liquid,
        has_entity_collision,
    )
}

fn ocelot_contains_any_liquid(
    world: &World,
    pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    if entity_type.id != EntityType::OCELOT.id {
        return false;
    }

    let bounding_box = BoundingBox::new_from_pos(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
        &EntityDimensions {
            width: entity_type.dimension[0],
            height: entity_type.dimension[1],
            eye_height: entity_type.eye_height,
        },
    );

    for x in bounding_box.min.x.floor() as i32..bounding_box.max.x.ceil() as i32 {
        for y in bounding_box.min.y.floor() as i32..bounding_box.max.y.ceil() as i32 {
            for z in bounding_box.min.z.floor() as i32..bounding_box.max.z.ceil() as i32 {
                let block_pos = BlockPos::new(x, y, z);
                if !world.get_fluid_and_fluid_state(&block_pos).1.is_empty {
                    return true;
                }
            }
        }
    }

    false
}

fn ocelot_has_entity_collision(
    world: &World,
    pos: &BlockPos,
    entity_type: &'static EntityType,
) -> bool {
    if entity_type.id != EntityType::OCELOT.id {
        return false;
    }

    let bounding_box = BoundingBox::new_from_pos(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
        &EntityDimensions {
            width: entity_type.dimension[0],
            height: entity_type.dimension[1],
            eye_height: entity_type.eye_height,
        },
    );

    world
        .get_all_at_box(&bounding_box.expand_all(1.0e-7))
        .iter()
        .any(|entity| !entity.is_spectator() && entity.can_be_collided_with())
}

/// `AbstractNautilus.checkNautilusSpawnRules`'s Y-range gate:
/// `pos.getY() >= seaLevel - 25 && pos.getY() <= seaLevel - 5`.
const fn is_in_nautilus_y_range(y: i32, sea_level: i32) -> bool {
    y >= sea_level - 25 && y <= sea_level - 5
}

/// `GlowSquid.checkGlowSquidSpawnRules`'s Y gate: `pos.getY() <= level.getSeaLevel() - 33`.
const fn is_below_glow_squid_y_threshold(y: i32, sea_level: i32) -> bool {
    y <= sea_level - 33
}

/// `Ocelot.checkOcelotSpawnRules`: `random.nextInt(3) != 0`.
const fn ocelot_spawn_roll_allowed(roll: u8) -> bool {
    roll != 0
}

#[allow(clippy::fn_params_excessive_bools)]
const fn ocelot_spawn_obstruction_allowed(
    y: i32,
    sea_level: i32,
    below_is_grass: bool,
    below_is_leaves: bool,
    contains_any_liquid: bool,
    has_entity_collision: bool,
) -> bool {
    !contains_any_liquid
        && !has_entity_collision
        && y >= sea_level
        && (below_is_grass || below_is_leaves)
}

#[cfg(test)]
mod animal_spawn_dispatch_tests {
    use super::{
        EntityType, ocelot_spawn_obstruction_allowed, ocelot_spawn_roll_allowed,
        uses_animal_spawn_rules, uses_any_light_monster_spawn_rules,
    };

    #[test]
    fn vanilla_animal_placements_use_the_animal_predicate() {
        for entity_type in [
            EntityType::CAT,
            EntityType::CHICKEN,
            EntityType::COW,
            EntityType::DONKEY,
            EntityType::HAPPY_GHAST,
            EntityType::HORSE,
            EntityType::LLAMA,
            EntityType::MULE,
            EntityType::PANDA,
            EntityType::PIG,
            EntityType::SHEEP,
            EntityType::SKELETON_HORSE,
            EntityType::TRADER_LLAMA,
        ] {
            assert!(uses_animal_spawn_rules(entity_type.id));
        }
    }

    #[test]
    fn any_light_monsters_skip_the_darkness_predicate() {
        assert!(uses_any_light_monster_spawn_rules(EntityType::BLAZE.id));
        assert!(uses_any_light_monster_spawn_rules(EntityType::BREEZE.id));
        assert!(uses_any_light_monster_spawn_rules(EntityType::ZOGLIN.id));
        assert!(!uses_any_light_monster_spawn_rules(EntityType::CREEPER.id));
    }

    #[test]
    fn unrelated_spawn_placements_do_not_use_the_animal_predicate() {
        assert!(!uses_animal_spawn_rules(EntityType::ZOMBIE.id));
        assert!(!uses_animal_spawn_rules(EntityType::BAT.id));
        assert!(!uses_animal_spawn_rules(EntityType::SLIME.id));
    }

    #[test]
    fn ocelot_spawn_roll_matches_vanilla() {
        assert!(!ocelot_spawn_roll_allowed(0));
        assert!(ocelot_spawn_roll_allowed(1));
        assert!(ocelot_spawn_roll_allowed(2));
    }

    #[test]
    fn ocelot_obstruction_matches_vanilla() {
        assert!(ocelot_spawn_obstruction_allowed(
            64, 63, true, false, false, false
        ));
        assert!(ocelot_spawn_obstruction_allowed(
            64, 63, false, true, false, false
        ));
        assert!(!ocelot_spawn_obstruction_allowed(
            62, 63, true, false, false, false
        ));
        assert!(!ocelot_spawn_obstruction_allowed(
            64, 63, false, false, false, false
        ));
        assert!(!ocelot_spawn_obstruction_allowed(
            64, 63, true, false, true, false
        ));
        assert!(!ocelot_spawn_obstruction_allowed(
            64, 63, true, false, false, true
        ));
    }
}

#[cfg(test)]
mod slime_spawn_dispatch_tests {
    use super::{
        EntityType, SpawnRuleContext, ghast_spawn_roll_allowed,
        nearby_player_restricted_any_light_spawn_rules_allowed,
        surface_monster_spawn_rules_allowed, uses_generic_monster_spawn_rules,
        uses_nearby_player_restricted_any_light_spawn_rules, uses_surface_monster_spawn_rules,
        zombified_piglin_spawn_rules_allowed,
    };
    use pumpkin_data::chunk::{Biome, NETHER_BIOME_SOURCE};

    /// Slime is `MobCategory::MONSTER`, so the category-wide branch in `check_spawn_rules`
    /// would otherwise return before the `EntityType::SLIME` branch is ever reached,
    /// leaving `SlimeEntity::check_slime_spawn_rules` dead.
    #[test]
    fn slime_is_excluded_from_the_generic_monster_branch() {
        assert!(!uses_generic_monster_spawn_rules(EntityType::SLIME.id));
    }

    #[test]
    fn other_monsters_still_use_the_generic_monster_branch() {
        assert!(uses_generic_monster_spawn_rules(EntityType::ZOMBIE.id));
        assert!(uses_generic_monster_spawn_rules(EntityType::CREEPER.id));
        assert!(uses_generic_monster_spawn_rules(EntityType::MAGMA_CUBE.id));
    }

    #[test]
    fn dedicated_monster_placements_skip_the_generic_branch() {
        assert!(!uses_generic_monster_spawn_rules(EntityType::HOGLIN.id));
    }

    #[test]
    fn endermites_and_silverfish_use_any_light_with_player_exclusion() {
        assert!(uses_nearby_player_restricted_any_light_spawn_rules(
            EntityType::ENDERMITE.id
        ));
        assert!(uses_nearby_player_restricted_any_light_spawn_rules(
            EntityType::SILVERFISH.id
        ));
        assert!(!uses_nearby_player_restricted_any_light_spawn_rules(
            EntityType::ZOMBIE.id
        ));
    }

    /// `WorldGenRegion` has no players, so vanilla's nearby-player rejection can only fire
    /// on the natural path. Pumpkin runs both paths against the live world, so the
    /// distinction has to come from the context argument.
    #[test]
    fn endermites_and_silverfish_only_apply_the_player_check_to_natural_spawns() {
        assert!(nearby_player_restricted_any_light_spawn_rules_allowed(
            SpawnRuleContext::ChunkGeneration,
            true,
            true,
        ));
        assert!(nearby_player_restricted_any_light_spawn_rules_allowed(
            SpawnRuleContext::Natural,
            true,
            false,
        ));
        assert!(!nearby_player_restricted_any_light_spawn_rules_allowed(
            SpawnRuleContext::Natural,
            true,
            true,
        ));
        assert!(!nearby_player_restricted_any_light_spawn_rules_allowed(
            SpawnRuleContext::Natural,
            false,
            false,
        ));
    }

    #[test]
    fn surface_monster_spawn_rule_dispatch_matches_java_registrations() {
        assert!(uses_surface_monster_spawn_rules(EntityType::CAMEL_HUSK.id));
        assert!(uses_surface_monster_spawn_rules(EntityType::HUSK.id));
        assert!(uses_surface_monster_spawn_rules(EntityType::PARCHED.id));
        assert!(!uses_surface_monster_spawn_rules(EntityType::SKELETON.id));
    }

    #[test]
    fn surface_monsters_require_darkness_and_open_sky() {
        assert!(surface_monster_spawn_rules_allowed(true, true));
        assert!(!surface_monster_spawn_rules_allowed(false, true));
        assert!(!surface_monster_spawn_rules_allowed(true, false));
    }

    #[test]
    fn ghasts_use_vanillas_one_in_twenty_roll() {
        assert!(ghast_spawn_roll_allowed(0));
        assert!(!ghast_spawn_roll_allowed(1));
        assert!(!ghast_spawn_roll_allowed(19));
    }

    #[test]
    fn zombified_piglins_ignore_darkness_but_reject_peaceful_and_nether_wart() {
        assert!(zombified_piglin_spawn_rules_allowed(true, true));
        assert!(!zombified_piglin_spawn_rules_allowed(false, true));
        assert!(!zombified_piglin_spawn_rules_allowed(true, false));
    }

    /// The reported Nether sighting cannot come from the biome spawn tables: no biome the
    /// Nether biome source can produce lists slime in any spawn group.
    #[test]
    fn no_nether_biome_can_offer_a_slime_spawner() {
        fn collect(tree: &pumpkin_data::chunk::BiomeTree, out: &mut Vec<&'static Biome>) {
            match tree {
                pumpkin_data::chunk::BiomeTree::Leaf { biome, .. } => out.push(biome),
                pumpkin_data::chunk::BiomeTree::Branch { nodes, .. } => {
                    for node in *nodes {
                        collect(node, out);
                    }
                }
            }
        }

        let mut biomes = Vec::new();
        collect(&NETHER_BIOME_SOURCE, &mut biomes);
        assert!(!biomes.is_empty());

        for biome in biomes {
            let groups = &biome.spawners;
            for group in [
                groups.monster,
                groups.creature,
                groups.ambient,
                groups.axolotls,
                groups.underground_water_creature,
                groups.water_creature,
                groups.water_ambient,
                groups.misc,
            ] {
                assert!(
                    !group.iter().any(|s| s.r#type == "minecraft:slime"),
                    "{} lists a slime spawner",
                    biome.registry_id
                );
            }
        }
    }
}

#[cfg(test)]
mod squid_spawn_rule_tests {
    use super::{
        is_below_glow_squid_y_threshold, is_in_nautilus_y_range, is_in_surface_water_y_range,
    };

    #[test]
    fn surface_squid_range_matches_vanilla_bounds() {
        let sea_level = 63;
        assert!(!is_in_surface_water_y_range(sea_level - 14, sea_level));
        assert!(is_in_surface_water_y_range(sea_level - 13, sea_level));
        assert!(is_in_surface_water_y_range(sea_level, sea_level));
        assert!(!is_in_surface_water_y_range(sea_level + 1, sea_level));
    }

    #[test]
    fn nautilus_range_matches_vanilla_bounds() {
        let sea_level = 63;
        assert!(!is_in_nautilus_y_range(sea_level - 26, sea_level));
        assert!(is_in_nautilus_y_range(sea_level - 25, sea_level));
        assert!(is_in_nautilus_y_range(sea_level - 5, sea_level));
        assert!(!is_in_nautilus_y_range(sea_level - 4, sea_level));
    }

    #[test]
    fn glow_squid_threshold_matches_vanilla_bound() {
        let sea_level = 63;
        assert!(!is_below_glow_squid_y_threshold(sea_level - 32, sea_level));
        assert!(is_below_glow_squid_y_threshold(sea_level - 33, sea_level));
        assert!(is_below_glow_squid_y_threshold(sea_level - 100, sea_level));
    }
}
