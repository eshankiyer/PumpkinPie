pub mod armor_stand;
pub mod arrow;
pub mod axe;
pub mod boat;
pub mod bone_meal;
pub mod bow;
pub mod brush;
pub mod bucket;
pub mod bundle;
pub mod clock;
pub mod compass;
pub mod crossbow;
pub mod dye;
pub mod egg;
pub mod end_crystal;
pub mod ender_eye;
pub mod ender_pearl;
pub mod experience_bottle;
pub mod firework_rocket;
pub mod fishing_rod;
pub mod glass_bottle;
pub mod glowing_ink_sac;
pub mod goat_horn;
pub mod hoe;
pub mod honeycomb;
pub mod ignite;
pub mod ink_sac;
pub mod item_frame;
pub mod knowledge_book;
pub mod lead;
pub mod mace;
pub mod map;
pub mod minecart;
pub mod name_tag;
pub mod on_a_stick;
pub mod painting;
pub mod potions;
pub mod saddle;
pub mod shears;
pub mod shovel;
pub mod snowball;
pub mod spawn_egg;
pub mod spear;
pub mod spyglass;
pub mod swords;
pub mod trident;
pub mod wind_charge;
pub mod writable_book;

use crate::item::items::armor_stand::ArmorStandItem;
use crate::item::items::boat::BoatItem;
use crate::item::items::bone_meal::BoneMealItem;
use crate::item::items::brush::BrushItem;
use crate::item::items::bundle::BundleItem;
use crate::item::items::clock::ClockItem;
use crate::item::items::compass::CompassItem;
use crate::item::items::end_crystal::EndCrystalItem;
use crate::item::items::experience_bottle::ExperienceBottleItem;
use crate::item::items::glass_bottle::GlassBottleItem;
use crate::item::items::goat_horn::GoatHornItem;
use crate::item::items::knowledge_book::KnowledgeBookItem;
use crate::item::items::lead::LeadItem;
use crate::item::items::map::MapItem;

use crate::item::items::minecart::MinecartItem;
use crate::item::items::name_tag::NameTagItem;
use crate::item::items::on_a_stick::{CarrotOnAStickItem, WarpedFungusOnAStickItem};
use crate::item::items::painting::PaintingItem;
use crate::item::items::saddle::SaddleItem;
use crate::item::items::shears::ShearsItem;
use crate::item::items::spawn_egg::SpawnEggItem;
use crate::item::items::spyglass::SpyglassItem;
use crate::item::items::wind_charge::WindChargeItem;
use crate::item::items::writable_book::WritableBookItem;
use firework_rocket::FireworkRocketItem;
use fishing_rod::FishingRodItem;
use glowing_ink_sac::GlowingInkSacItem;
use pumpkin_data::{Block, BlockStateId};

use super::registry::ItemRegistry;
use crate::item::items::potions::{LingeringPotionItem, PotionItem, SplashPotionItem};
use arrow::ArrowItem;
use axe::AxeItem;
use bow::BowItem;
use bucket::{EmptyBucketItem, FilledBucketItem, MilkBucketItem};
use crossbow::CrossbowItem;
use dye::DyeItem;
use egg::EggItem;
use ender_eye::EnderEyeItem;
use ender_pearl::EnderPearlItem;
use hoe::HoeItem;
use honeycomb::HoneyCombItem;
use ignite::fire_charge::FireChargeItem;
use ignite::flint_and_steel::FlintAndSteelItem;
use ink_sac::InkSacItem;
use item_frame::ItemFrameItem;
use mace::MaceItem;
use shovel::ShovelItem;
use snowball::SnowBallItem;
use spear::SpearItem;
use std::sync::Arc;
use swords::SwordItem;
use trident::TridentItem;

/// Pitch shared by the vanilla throw sounds of `SnowballItem`, `EggItem`,
/// `ExperienceBottleItem` and `FishingRodItem`: `0.4F / (random.nextFloat() * 0.4F + 0.8F)`.
#[must_use]
pub fn throw_sound_pitch(random_value: f32) -> f32 {
    0.4 / (random_value * 0.4 + 0.8)
}

/// Returns the state of `new_block` that carries over every property it shares
/// with `old_block` in `old_state_id`.
///
/// Equivalent to vanilla's `Block#withPropertiesOf`, used by stripping, waxing,
/// scraping and de-oxidizing to keep facing, waterlogging, slab type, and so on.
/// Falls back to the default state when either block carries no properties.
#[must_use]
pub fn state_with_properties_of(
    old_block: &Block,
    old_state_id: BlockStateId,
    new_block: &Block,
) -> BlockStateId {
    let default_state_id = new_block.default_state.id;
    if new_block.properties(default_state_id).is_none() {
        return default_state_id;
    }
    old_block
        .properties(old_state_id)
        .map_or(default_state_id, |properties| {
            new_block
                .from_properties(&properties.to_props())
                .to_state_id(new_block)
        })
}

#[must_use]
pub fn default_registry() -> Arc<ItemRegistry> {
    let mut manager = ItemRegistry::default();

    manager.register(ArrowItem);
    manager.register(KnowledgeBookItem);
    manager.register(PaintingItem);
    manager.register(BowItem);
    manager.register(BoneMealItem);
    manager.register(CrossbowItem);
    manager.register(SnowBallItem);
    manager.register(HoeItem);
    manager.register(EggItem);
    manager.register(FlintAndSteelItem);
    manager.register(SwordItem);
    manager.register(MaceItem);
    manager.register(TridentItem);
    manager.register(SpearItem::new());
    manager.register(FishingRodItem);
    manager.register(BrushItem);
    manager.register(CarrotOnAStickItem);
    manager.register(WarpedFungusOnAStickItem);
    manager.register(ShearsItem);
    manager.register(SaddleItem);
    manager.register(BoneMealItem);
    manager.register(GlassBottleItem);
    manager.register(ExperienceBottleItem);
    manager.register(CompassItem);
    manager.register(ClockItem);
    manager.register(WritableBookItem);
    manager.register(SpyglassItem);
    manager.register(GoatHornItem);
    manager.register(EmptyBucketItem);
    manager.register(FilledBucketItem);
    manager.register(MilkBucketItem);
    manager.register(ShovelItem);
    manager.register(SpawnEggItem);
    manager.register(AxeItem);
    manager.register(EndCrystalItem);
    manager.register(MinecartItem);
    manager.register(HoneyCombItem);
    manager.register(NameTagItem);
    manager.register(EnderEyeItem);
    manager.register(EnderPearlItem);
    manager.register(ExperienceBottleItem);
    manager.register(FireChargeItem);
    manager.register(DyeItem);
    manager.register(MapItem);
    manager.register(FireworkRocketItem);
    manager.register(InkSacItem);
    manager.register(GlowingInkSacItem);
    manager.register(ArmorStandItem);
    manager.register(WindChargeItem);
    manager.register(BoatItem);
    manager.register(PotionItem);
    manager.register(SplashPotionItem);
    manager.register(LingeringPotionItem);
    manager.register(BundleItem);
    manager.register(CompassItem);
    manager.register(LeadItem);
    manager.register(SpyglassItem);
    manager.register(ShearsItem);
    manager.register(ItemFrameItem);

    Arc::new(manager)
}
