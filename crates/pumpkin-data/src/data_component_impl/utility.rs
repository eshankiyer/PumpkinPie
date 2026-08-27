use crate::data_component_impl::{DataComponentImpl, get_i32_hash, get_str_hash};
use crc_fast::CrcAlgorithm::Crc32Iscsi;
use crc_fast::Digest;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use std::borrow::Cow;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct DyeImpl;
impl DataComponentImpl for DyeImpl {
    default_impl!(Dye);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct DyedColorImpl {
    pub rgb: i32,
}
impl DyedColorImpl {
    pub fn read_data(data: &NbtTag) -> Option<Self> {
        data.extract_int().map(|rgb| Self { rgb })
    }
}
impl DataComponentImpl for DyedColorImpl {
    fn write_data(&self) -> NbtTag {
        NbtTag::Int(self.rgb)
    }
    fn get_hash(&self) -> i32 {
        get_i32_hash(self.rgb) as i32
    }
    default_impl!(DyedColor);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct MapColorImpl;
impl DataComponentImpl for MapColorImpl {
    default_impl!(MapColor);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct MapIdImpl {
    pub id: i32,
}
impl MapIdImpl {
    pub fn read_data(data: &NbtTag) -> Option<Self> {
        data.extract_int().map(|id| Self { id })
    }
}
impl DataComponentImpl for MapIdImpl {
    fn write_data(&self) -> NbtTag {
        NbtTag::Int(self.id)
    }
    fn get_hash(&self) -> i32 {
        get_i32_hash(self.id) as i32
    }
    default_impl!(MapId);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct MapDecorationsImpl;
impl DataComponentImpl for MapDecorationsImpl {
    default_impl!(MapDecorations);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct MapPostProcessingImpl;
impl DataComponentImpl for MapPostProcessingImpl {
    default_impl!(MapPostProcessing);
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChargedProjectilesImpl {
    pub projectiles: Vec<NbtCompound>,
}
impl ChargedProjectilesImpl {
    pub fn read_data(data: &NbtTag) -> Option<Self> {
        let list = data.extract_list()?;
        let mut projectiles = Vec::new();
        for item in list {
            projectiles.push(item.extract_compound()?.clone());
        }
        Some(Self { projectiles })
    }
}
impl DataComponentImpl for ChargedProjectilesImpl {
    fn write_data(&self) -> NbtTag {
        let mut list = Vec::new();
        for item in &self.projectiles {
            list.push(NbtTag::Compound(item.clone()));
        }
        NbtTag::List(list)
    }
    fn get_hash(&self) -> i32 {
        0
    }
    default_impl!(ChargedProjectiles);
}

#[derive(Clone)]
pub struct BundleContentsImpl {
    pub items: Vec<crate::item_stack::ItemStack>,
    /// Vanilla `BundleContents.selectedItem`; -1 means no selected item.
    pub selected_item: i32,
}
impl PartialEq for BundleContentsImpl {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
impl Eq for BundleContentsImpl {}
impl std::fmt::Debug for BundleContentsImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BundleContentsImpl")
    }
}
impl BundleContentsImpl {
    pub fn read_data(tag: &NbtTag) -> Option<Self> {
        let mut items = Vec::new();
        if let NbtTag::List(l) = tag {
            for item_tag in l {
                if let NbtTag::Compound(c) = item_tag
                    && let Some(stack) = crate::item_stack::ItemStack::read_item_stack(c)
                {
                    items.push(stack);
                }
            }
        }
        Some(Self {
            items,
            selected_item: -1,
        })
    }
    pub fn get_weight(&self) -> u32 {
        self.items
            .iter()
            .map(|item| item.item_count as u32 * (64 / item.get_max_stack_size() as u32).max(1))
            .sum()
    }
    pub fn try_insert(&mut self, stack: &mut crate::item_stack::ItemStack) -> bool {
        if stack.is_empty() || stack.get_data_component::<BundleContentsImpl>().is_some() {
            return false;
        }
        let weight_per_item = (64 / stack.get_max_stack_size() as u32).max(1);
        let mut inserted_anything = false;
        while stack.item_count > 0 && self.get_weight() + weight_per_item <= 64 {
            if let Some(top) = self.items.first_mut()
                && crate::item_stack::ItemStack::are_items_and_components_equal(top, stack)
                && top.item_count < top.get_max_stack_size()
            {
                top.item_count += 1;
                stack.item_count -= 1;
                inserted_anything = true;
                continue;
            }
            self.items.insert(0, stack.copy_with_count(1));
            stack.item_count -= 1;
            inserted_anything = true;
        }
        inserted_anything
    }
    pub fn try_extract(&mut self) -> Option<crate::item_stack::ItemStack> {
        if self.items.is_empty() {
            None
        } else {
            let index = usize::try_from(self.selected_item)
                .ok()
                .filter(|&index| index < self.items.len())
                .unwrap_or(0);
            self.selected_item = -1;
            Some(self.items.remove(index))
        }
    }

    /// `BundleContents.Mutable.toggleSelectedItem` (`BundleContents.java:228-234`).
    pub fn toggle_selected_item(&mut self, selected_item: i32) {
        let valid = usize::try_from(selected_item)
            .ok()
            .is_some_and(|index| index < self.items.len());
        self.selected_item = if self.selected_item != selected_item && valid {
            selected_item
        } else {
            -1
        };
    }

    /// `BundleContents.getSelectedItemIndex` (`BundleContents.java:111-113`).
    #[must_use]
    pub const fn selected_item_index(&self) -> i32 {
        self.selected_item
    }
}
impl DataComponentImpl for BundleContentsImpl {
    fn write_data(&self) -> NbtTag {
        let mut list = Vec::new();
        for stack in &self.items {
            let mut item_compound = NbtCompound::new();
            stack.write_item_stack(&mut item_compound);
            list.push(NbtTag::Compound(item_compound));
        }
        NbtTag::List(list)
    }
    default_impl!(BundleContents);
}

/// The dimension and block position a lodestone compass points to.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LodestoneTarget {
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LodestoneTrackerImpl {
    pub target: Option<LodestoneTarget>,
    pub tracked: bool,
}
impl LodestoneTrackerImpl {
    pub fn read_data(data: &NbtTag) -> Option<Self> {
        let compound = data.extract_compound()?;
        let tracked = compound.get_bool("tracked").unwrap_or(true);
        let target = compound.get_compound("target").and_then(|target| {
            let dimension = target.get_string("dimension")?.to_string();
            let pos = target.get_int_array("pos")?;
            if pos.len() != 3 {
                return None;
            }
            Some(LodestoneTarget {
                dimension,
                x: pos[0],
                y: pos[1],
                z: pos[2],
            })
        });
        Some(Self { target, tracked })
    }
}
impl DataComponentImpl for LodestoneTrackerImpl {
    fn write_data(&self) -> NbtTag {
        let mut compound = NbtCompound::new();
        if let Some(target) = &self.target {
            let mut target_compound = NbtCompound::new();
            target_compound.put_string("dimension", target.dimension.clone());
            target_compound.put("pos", NbtTag::IntArray(vec![target.x, target.y, target.z]));
            compound.put_compound("target", target_compound);
        }
        compound.put_bool("tracked", self.tracked);
        NbtTag::Compound(compound)
    }
    default_impl!(LodestoneTracker);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FireworkExplosionShape {
    SmallBall = 0,
    LargeBall = 1,
    Star = 2,
    Creeper = 3,
    Burst = 4,
}
impl FireworkExplosionShape {
    pub fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::SmallBall),
            1 => Some(Self::LargeBall),
            2 => Some(Self::Star),
            3 => Some(Self::Creeper),
            4 => Some(Self::Burst),
            _ => None,
        }
    }
    pub fn to_id(&self) -> i32 {
        *self as i32
    }
    pub fn to_name(&self) -> &str {
        match self {
            Self::SmallBall => "small_ball",
            Self::LargeBall => "large_ball",
            Self::Star => "star",
            Self::Creeper => "creeper",
            Self::Burst => "burst",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "small_ball" => Some(Self::SmallBall),
            "large_ball" => Some(Self::LargeBall),
            "star" => Some(Self::Star),
            "creeper" => Some(Self::Creeper),
            "burst" => Some(Self::Burst),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct FireworkExplosionImpl {
    pub shape: FireworkExplosionShape,
    pub colors: Vec<i32>,
    pub fade_colors: Vec<i32>,
    pub has_trail: bool,
    pub has_twinkle: bool,
}
impl FireworkExplosionImpl {
    pub fn new(
        shape: FireworkExplosionShape,
        colors: Vec<i32>,
        fade_colors: Vec<i32>,
        has_trail: bool,
        has_twinkle: bool,
    ) -> Self {
        Self {
            shape,
            colors,
            fade_colors,
            has_trail,
            has_twinkle,
        }
    }
    pub fn read_data(tag: &NbtTag) -> Option<Self> {
        let compound = tag.extract_compound()?;
        let shape = FireworkExplosionShape::from_name(compound.get_string("shape")?)?;
        let colors = compound
            .get_int_array("colors")
            .map(|v| v.to_vec())
            .unwrap_or_default();
        let fade_colors = compound
            .get_int_array("fade_colors")
            .map(|v| v.to_vec())
            .unwrap_or_default();
        let has_trail = compound.get_bool("has_trail").unwrap_or(false);
        let has_twinkle = compound.get_bool("has_twinkle").unwrap_or(false);
        Some(Self {
            shape,
            colors,
            fade_colors,
            has_trail,
            has_twinkle,
        })
    }
}
impl DataComponentImpl for FireworkExplosionImpl {
    fn write_data(&self) -> NbtTag {
        let mut compound = NbtCompound::new();
        compound.put_string("shape", self.shape.to_name().to_string());
        compound.put("colors", NbtTag::IntArray(self.colors.clone()));
        compound.put("fade_colors", NbtTag::IntArray(self.fade_colors.clone()));
        compound.put_bool("has_trail", self.has_trail);
        compound.put_bool("has_twinkle", self.has_twinkle);
        NbtTag::Compound(compound)
    }
    fn get_hash(&self) -> i32 {
        let mut digest = Digest::new(Crc32Iscsi);
        digest.update(&[2u8]);
        digest.update(&[self.shape.to_id() as u8]);
        for color in &self.colors {
            digest.update(&get_i32_hash(*color).to_le_bytes());
        }
        digest.update(&[3u8]);
        for color in &self.fade_colors {
            digest.update(&get_i32_hash(*color).to_le_bytes());
        }
        digest.update(&[4u8]);
        digest.update(&[self.has_trail as u8]);
        digest.update(&[self.has_twinkle as u8]);
        digest.finalize() as i32
    }
    default_impl!(FireworkExplosion);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct FireworksImpl {
    pub flight_duration: i32,
    pub explosions: Vec<FireworkExplosionImpl>,
}
impl FireworksImpl {
    pub fn new(flight_duration: i32, explosions: Vec<FireworkExplosionImpl>) -> Self {
        Self {
            flight_duration,
            explosions,
        }
    }
    pub fn read_data(tag: &NbtTag) -> Option<Self> {
        let compound = tag.extract_compound()?;
        let flight_duration = compound
            .get_byte("flight_duration")
            .map(i32::from)
            .or_else(|| compound.get_int("flight_duration"))
            .unwrap_or(1);
        let mut explosions = Vec::new();
        if let Some(list) = compound.get_list("explosions") {
            for item in list {
                if let Some(explosion) = FireworkExplosionImpl::read_data(item) {
                    explosions.push(explosion);
                }
            }
        }
        Some(Self {
            flight_duration,
            explosions,
        })
    }
}
impl DataComponentImpl for FireworksImpl {
    fn write_data(&self) -> NbtTag {
        let mut compound = NbtCompound::new();
        compound.put_int("flight_duration", self.flight_duration);
        let explosions_list: Vec<NbtTag> = self.explosions.iter().map(|e| e.write_data()).collect();
        compound.put_list("explosions", explosions_list);
        NbtTag::Compound(compound)
    }
    fn get_hash(&self) -> i32 {
        let mut digest = Digest::new(Crc32Iscsi);
        digest.update(&[2u8]);
        digest.update(&get_i32_hash(self.flight_duration).to_le_bytes());
        for explosion in &self.explosions {
            digest.update(&get_i32_hash(explosion.get_hash()).to_le_bytes());
        }
        digest.update(&[3u8]);
        digest.finalize() as i32
    }
    default_impl!(Fireworks);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Default)]
pub struct ProfileImpl {
    pub name: Option<String>,
    pub id: Option<[i32; 4]>,
    pub properties: Vec<ProfileProperty>,
    pub texture: Option<String>,
    pub cape: Option<String>,
    pub elytra: Option<String>,
    pub model: Option<String>,
}
impl ProfileImpl {
    pub fn read_data(data: &NbtTag) -> Option<Self> {
        match data {
            NbtTag::String(name) => Some(Self {
                name: Some(name.to_string()),
                ..Default::default()
            }),
            NbtTag::Compound(compound) => {
                let name = compound.get_string("name").map(String::from);
                let id = compound.get_int_array("id").and_then(|arr| {
                    if arr.len() == 4 {
                        Some([arr[0], arr[1], arr[2], arr[3]])
                    } else {
                        None
                    }
                });
                let mut properties = Vec::new();
                if let Some(props_list) = compound.get_list("properties") {
                    for prop_tag in props_list {
                        if let Some(prop_comp) = prop_tag.extract_compound()
                            && let (Some(prop_name), Some(prop_value)) =
                                (prop_comp.get_string("name"), prop_comp.get_string("value"))
                        {
                            properties.push(ProfileProperty {
                                name: prop_name.to_string(),
                                value: prop_value.to_string(),
                                signature: prop_comp.get_string("signature").map(String::from),
                            });
                        }
                    }
                }
                let texture = compound.get_string("texture").map(String::from);
                let cape = compound.get_string("cape").map(String::from);
                let elytra = compound.get_string("elytra").map(String::from);
                let model = compound.get_string("model").map(String::from);
                Some(Self {
                    name,
                    id,
                    properties,
                    texture,
                    cape,
                    elytra,
                    model,
                })
            }
            _ => None,
        }
    }
}
impl DataComponentImpl for ProfileImpl {
    fn write_data(&self) -> NbtTag {
        let mut compound = NbtCompound::new();
        if let Some(name) = &self.name {
            compound.put_string("name", name.clone());
        }
        if let Some(id) = &self.id {
            compound.put("id", NbtTag::IntArray(id.to_vec()));
        }
        if !self.properties.is_empty() {
            let mut props_list = Vec::new();
            for prop in &self.properties {
                let mut prop_comp = NbtCompound::new();
                prop_comp.put_string("name", prop.name.clone());
                prop_comp.put_string("value", prop.value.clone());
                if let Some(sig) = &prop.signature {
                    prop_comp.put_string("signature", sig.clone());
                }
                props_list.push(NbtTag::Compound(prop_comp));
            }
            compound.put_list("properties", props_list);
        }
        if let Some(texture) = &self.texture {
            compound.put_string("texture", texture.clone());
        }
        if let Some(cape) = &self.cape {
            compound.put_string("cape", cape.clone());
        }
        if let Some(elytra) = &self.elytra {
            compound.put_string("elytra", elytra.clone());
        }
        if let Some(model) = &self.model {
            compound.put_string("model", model.clone());
        }
        NbtTag::Compound(compound)
    }
    fn get_hash(&self) -> i32 {
        let mut digest = Digest::new(Crc32Iscsi);
        if let Some(name) = &self.name {
            digest.update(&[1u8]);
            digest.update(&get_str_hash(name).to_le_bytes());
        }
        if let Some(id) = &self.id {
            digest.update(&[2u8]);
            for val in id {
                digest.update(&get_i32_hash(*val).to_le_bytes());
            }
        }
        if !self.properties.is_empty() {
            digest.update(&[3u8]);
            for prop in &self.properties {
                digest.update(&get_str_hash(&prop.name).to_le_bytes());
                digest.update(&get_str_hash(&prop.value).to_le_bytes());
                if let Some(sig) = &prop.signature {
                    digest.update(&[1u8]);
                    digest.update(&get_str_hash(sig).to_le_bytes());
                } else {
                    digest.update(&[0u8]);
                }
            }
        }
        if let Some(texture) = &self.texture {
            digest.update(&[4u8]);
            digest.update(&get_str_hash(texture).to_le_bytes());
        }
        if let Some(cape) = &self.cape {
            digest.update(&[5u8]);
            digest.update(&get_str_hash(cape).to_le_bytes());
        }
        if let Some(elytra) = &self.elytra {
            digest.update(&[6u8]);
            digest.update(&get_str_hash(elytra).to_le_bytes());
        }
        if let Some(model) = &self.model {
            digest.update(&[7u8]);
            digest.update(&get_str_hash(model).to_le_bytes());
        }
        digest.finalize() as i32
    }
    default_impl!(Profile);
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct JukeboxPlayableImpl {
    pub song: &'static str,
}
impl DataComponentImpl for JukeboxPlayableImpl {
    default_impl!(JukeboxPlayable);
}

/// `DataComponents.RECIPES` (`DataComponents.java:290-292`): a plain list of recipe
/// keys, persisted with `Recipe.KEY_CODEC.listOf()` (`Recipe.java:20`, a
/// `ResourceKey` codec, i.e. a string). The type declares no
/// `networkSynchronized` codec, so its wire form is the `fromCodecWithRegistries`
/// fallback (`DataComponentType.java:70-76`) - the NBT tag itself.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct RecipesImpl {
    pub recipes: Cow<'static, [Cow<'static, str>]>,
}

/// The empty list, so that `&RecipesImpl` keeps naming a valid `'static` value in
/// the generated item table (`generated/item.rs`), which is not editable here.
/// A struct and a const may share a name because they live in different namespaces.
#[allow(non_upper_case_globals)]
pub const RecipesImpl: RecipesImpl = RecipesImpl {
    recipes: Cow::Borrowed(&[]),
};

impl RecipesImpl {
    #[must_use]
    pub fn from_ids(ids: Vec<String>) -> Self {
        Self {
            recipes: Cow::Owned(ids.into_iter().map(Cow::Owned).collect()),
        }
    }

    pub fn read_data(data: &NbtTag) -> Option<Self> {
        let list = data.extract_list()?;
        let mut ids = Vec::with_capacity(list.len());
        for tag in list {
            ids.push(Cow::Owned(tag.extract_string()?.to_owned()));
        }
        Some(Self {
            recipes: Cow::Owned(ids),
        })
    }
}

impl DataComponentImpl for RecipesImpl {
    fn write_data(&self) -> NbtTag {
        NbtTag::List(
            self.recipes
                .iter()
                .map(|id| NbtTag::String(id.as_ref().into()))
                .collect(),
        )
    }
    /// `HashOps.createList` (`HashOps.java:153-159`): tag 4, each element hash as
    /// little-endian bytes, tag 5. Elements are `HashOps.createString`
    /// (`HashOps.java:123-125`), which is what [`get_str_hash`] implements.
    fn get_hash(&self) -> i32 {
        let mut digest = Digest::new(Crc32Iscsi);
        digest.update(&[4u8]);
        for id in self.recipes.iter() {
            digest.update(&get_str_hash(id).to_le_bytes());
        }
        digest.update(&[5u8]);
        digest.finalize() as i32
    }
    default_impl!(Recipes);
}

#[cfg(test)]
mod tests {
    use super::{DataComponentImpl, RecipesImpl};
    use pumpkin_nbt::tag::NbtTag;
    use std::borrow::Cow;

    /// `Recipe.KEY_CODEC.listOf()` persists as a plain NBT list of strings.
    #[test]
    fn recipes_round_trip_through_nbt() {
        let value = RecipesImpl {
            recipes: Cow::Owned(vec![
                Cow::Borrowed("minecraft:stick"),
                Cow::Borrowed("minecraft:torch"),
            ]),
        };
        let tag = value.write_data();
        assert_eq!(
            tag,
            NbtTag::List(vec![
                NbtTag::String("minecraft:stick".into()),
                NbtTag::String("minecraft:torch".into()),
            ])
        );
        assert_eq!(RecipesImpl::read_data(&tag), Some(value));
    }

    /// The const of the same name keeps `&RecipesImpl` valid in the generated item
    /// table, and must stay the empty list vanilla's `getOrDefault` falls back to.
    #[test]
    fn the_default_recipes_component_is_empty() {
        assert!(super::RecipesImpl.recipes.is_empty());
    }
}
