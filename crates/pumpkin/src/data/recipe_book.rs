//! Per-player recipe book state.
//!
//! Port of vanilla's `RecipeBook` (`RecipeBook.java:5-36`), `RecipeBookSettings`
//! (`RecipeBookSettings.java:14-152`) and `ServerRecipeBook`
//! (`ServerRecipeBook.java:27-159`).
//!
//! Recipes are identified by their namespaced id string, standing in for vanilla's
//! `ResourceKey<Recipe<?>>`. Cooking recipes carry a real vanilla id
//! (`CookingRecipe::recipe_id`); the generated crafting table carries no id at all, so
//! the id of a crafting recipe is derived from its result (see [`RecipeRegistry`]).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use pumpkin_data::item::Item;
use pumpkin_data::recipes::{
    CookingRecipeType, CraftingRecipeTypes, RECIPES_COOKING, RECIPES_CRAFTING,
    RECIPES_STONECUTTING, RecipeIngredientTypes, RecipeResultStruct,
};
use pumpkin_data::tag::{RegistryKey, get_tag_ids};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;

/// The four recipe book tabs, `RecipeBookType.java`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeBookType {
    Crafting,
    Furnace,
    BlastFurnace,
    Smoker,
}

impl RecipeBookType {
    /// Wire order of `RecipeBookType`, as read by
    /// `ServerboundRecipeBookChangeSettingsPacket`.
    #[must_use]
    pub const fn from_id(id: i32) -> Option<Self> {
        match id {
            0 => Some(Self::Crafting),
            1 => Some(Self::Furnace),
            2 => Some(Self::BlastFurnace),
            3 => Some(Self::Smoker),
            _ => None,
        }
    }
}

/// `RecipeBookSettings.TypeSettings` (`RecipeBookSettings.java:114-151`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeSettings {
    pub open: bool,
    pub filtering: bool,
}

/// The per-tab NBT field names, `RecipeBookSettings.java:116-121`.
const SETTINGS_FIELDS: [(&str, &str); 4] = [
    ("isGuiOpen", "isFilteringCraftable"),
    ("isFurnaceGuiOpen", "isFurnaceFilteringCraftable"),
    (
        "isBlastingFurnaceGuiOpen",
        "isBlastingFurnaceFilteringCraftable",
    ),
    ("isSmokerGuiOpen", "isSmokerFilteringCraftable"),
];

/// `RecipeBookSettings` (`RecipeBookSettings.java:14-152`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookSettings {
    /// Indexed by [`RecipeBookType`] wire order.
    tabs: [TypeSettings; 4],
}

impl RecipeBookSettings {
    const fn index(book_type: RecipeBookType) -> usize {
        match book_type {
            RecipeBookType::Crafting => 0,
            RecipeBookType::Furnace => 1,
            RecipeBookType::BlastFurnace => 2,
            RecipeBookType::Smoker => 3,
        }
    }

    #[must_use]
    pub const fn settings(&self, book_type: RecipeBookType) -> TypeSettings {
        self.tabs[Self::index(book_type)]
    }

    #[must_use]
    pub const fn is_open(&self, book_type: RecipeBookType) -> bool {
        self.tabs[Self::index(book_type)].open
    }

    pub const fn set_open(&mut self, book_type: RecipeBookType, open: bool) {
        self.tabs[Self::index(book_type)].open = open;
    }

    #[must_use]
    pub const fn is_filtering(&self, book_type: RecipeBookType) -> bool {
        self.tabs[Self::index(book_type)].filtering
    }

    pub const fn set_filtering(&mut self, book_type: RecipeBookType, filtering: bool) {
        self.tabs[Self::index(book_type)].filtering = filtering;
    }

    /// `RecipeBook.setBookSetting` (`RecipeBook.java:32-35`).
    pub const fn set_book_setting(
        &mut self,
        book_type: RecipeBookType,
        open: bool,
        filtering: bool,
    ) {
        self.set_open(book_type, open);
        self.set_filtering(book_type, filtering);
    }

    /// The eight booleans of the clientbound settings packet, in wire order.
    #[must_use]
    pub const fn to_wire(self) -> [bool; 8] {
        [
            self.tabs[0].open,
            self.tabs[0].filtering,
            self.tabs[1].open,
            self.tabs[1].filtering,
            self.tabs[2].open,
            self.tabs[2].filtering,
            self.tabs[3].open,
            self.tabs[3].filtering,
        ]
    }

    fn write_nbt(self, nbt: &mut NbtCompound) {
        for (index, (open_field, filter_field)) in SETTINGS_FIELDS.iter().enumerate() {
            nbt.put_bool(open_field, self.tabs[index].open);
            nbt.put_bool(filter_field, self.tabs[index].filtering);
        }
    }

    fn read_nbt(nbt: &NbtCompound) -> Self {
        let mut settings = Self::default();
        for (index, (open_field, filter_field)) in SETTINGS_FIELDS.iter().enumerate() {
            settings.tabs[index] = TypeSettings {
                open: nbt.get_bool(open_field).unwrap_or(false),
                filtering: nbt.get_bool(filter_field).unwrap_or(false),
            };
        }
        settings
    }
}

/// NBT key the whole book is stored under on the player, `ServerPlayer.java:397,421`
/// (`ServerRecipeBook.RECIPE_BOOK_TAG`, `ServerRecipeBook.java:28`).
const RECIPE_BOOK_TAG: &str = "recipeBook";
/// `ServerRecipeBook.Packed.CODEC` field names (`ServerRecipeBook.java:154-155`).
const KNOWN_TAG: &str = "recipes";
const HIGHLIGHT_TAG: &str = "toBeDisplayed";

/// `ServerRecipeBook` (`ServerRecipeBook.java:27-159`): the unlocked set, the
/// not-yet-seen ("highlight") subset, and the tab settings.
#[derive(Clone, Debug, Default)]
pub struct ServerRecipeBook {
    settings: RecipeBookSettings,
    known: HashSet<String>,
    highlight: HashSet<String>,
}

impl ServerRecipeBook {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn settings(&self) -> RecipeBookSettings {
        self.settings
    }

    pub const fn set_settings(&mut self, settings: RecipeBookSettings) {
        self.settings = settings;
    }

    pub const fn settings_mut(&mut self) -> &mut RecipeBookSettings {
        &mut self.settings
    }

    /// `ServerRecipeBook.add` (`ServerRecipeBook.java:40-42`).
    pub fn add(&mut self, id: &str) {
        self.known.insert(id.to_owned());
    }

    /// `ServerRecipeBook.contains` (`ServerRecipeBook.java:44-46`).
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.known.contains(id)
    }

    /// `ServerRecipeBook.remove` (`ServerRecipeBook.java:48-51`).
    pub fn remove(&mut self, id: &str) {
        self.known.remove(id);
        self.highlight.remove(id);
    }

    /// `ServerRecipeBook.removeHighlight` (`ServerRecipeBook.java:53-55`).
    pub fn remove_highlight(&mut self, id: &str) {
        self.highlight.remove(id);
    }

    #[must_use]
    pub fn is_highlighted(&self, id: &str) -> bool {
        self.highlight.contains(id)
    }

    #[must_use]
    pub const fn known(&self) -> &HashSet<String> {
        &self.known
    }

    /// `ServerRecipeBook.addRecipes` (`ServerRecipeBook.java:61-80`) without the
    /// packet half: unlocks every id not already known and marks it highlighted,
    /// returning the ids that were newly unlocked.
    ///
    /// Vanilla skips `recipe.value().isSpecial()` recipes; the generated table models
    /// those as [`CraftingRecipeTypes::CraftingSpecial`] and
    /// [`CraftingRecipeTypes::CraftingDecoratedPot`], which [`RecipeRegistry`] never
    /// assigns an id to, so they can never reach this method.
    pub fn add_recipes<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut added = Vec::new();
        for id in ids {
            if !self.known.contains(id) {
                self.known.insert(id.to_owned());
                self.highlight.insert(id.to_owned());
                added.push(id.to_owned());
            }
        }
        added
    }

    /// `ServerRecipeBook.removeRecipes` (`ServerRecipeBook.java:82-98`).
    pub fn remove_recipes<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut removed = Vec::new();
        for id in ids {
            if self.known.contains(id) {
                self.remove(id);
                removed.push(id.to_owned());
            }
        }
        removed
    }

    /// `ServerRecipeBook.copyOverData` (`ServerRecipeBook.java:123-137`): replaces
    /// this book's whole state with another's, used when a player is recreated on
    /// respawn or dimension change.
    pub fn copy_over_data(&mut self, other: &Self) {
        self.settings = other.settings;
        self.known.clone_from(&other.known);
        self.highlight.clone_from(&other.highlight);
    }

    /// `ServerPlayer.java:421` storing `ServerRecipeBook.Packed.CODEC`.
    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        let mut book = NbtCompound::new();
        self.settings.write_nbt(&mut book);
        let mut known: Vec<&str> = self.known.iter().map(String::as_str).collect();
        known.sort_unstable();
        let mut highlight: Vec<&str> = self.highlight.iter().map(String::as_str).collect();
        highlight.sort_unstable();
        book.put_list(
            KNOWN_TAG,
            known
                .into_iter()
                .map(|id| NbtTag::String(id.into()))
                .collect(),
        );
        book.put_list(
            HIGHLIGHT_TAG,
            highlight
                .into_iter()
                .map(|id| NbtTag::String(id.into()))
                .collect(),
        );
        nbt.put_compound(RECIPE_BOOK_TAG, book);
    }

    /// `ServerRecipeBook.loadUntrusted` (`ServerRecipeBook.java:139-143`) plus the
    /// `loadRecipes` validator (`ServerRecipeBook.java:100-110`): ids the registry
    /// does not recognise are dropped rather than kept.
    pub fn read_nbt(&mut self, nbt: &NbtCompound) {
        let Some(book) = nbt.get_compound(RECIPE_BOOK_TAG) else {
            return;
        };
        self.settings = RecipeBookSettings::read_nbt(book);
        self.known = read_id_list(book, KNOWN_TAG);
        self.highlight = read_id_list(book, HIGHLIGHT_TAG);
        // A highlighted recipe that is not known is meaningless.
        self.highlight.retain(|id| self.known.contains(id));
    }
}

fn read_id_list(nbt: &NbtCompound, key: &str) -> HashSet<String> {
    let registry = registry();
    nbt.get_list(key)
        .unwrap_or(&[])
        .iter()
        .filter_map(|tag| match tag {
            NbtTag::String(id) => Some(id.to_string()),
            _ => None,
        })
        .filter(|id| {
            if registry.contains(id) {
                true
            } else {
                tracing::warn!("Tried to load unrecognized recipe: {id} removed now.");
                false
            }
        })
        .collect()
}

/// One entry of the server's static recipe table.
#[derive(Debug)]
pub struct RecipeEntry {
    /// Namespaced id, standing in for `ResourceKey<Recipe<?>>`.
    pub id: String,
    /// Index the client knows this recipe by, matching the order
    /// `CRecipeBookAdd` writes entries in (all crafting recipes except the special
    /// ones, then all cooking recipes).
    pub display_id: i32,
    /// Item id of the recipe result.
    pub result: u16,
}

/// Index over the generated recipe tables: id lookup, display-id lookup, and the
/// two "you obtained an item" reverse indices the unlock triggers need.
#[derive(Debug, Default)]
pub struct RecipeRegistry {
    entries: Vec<RecipeEntry>,
    by_id: HashMap<String, usize>,
    by_display_id: HashMap<i32, usize>,
    by_result: HashMap<u16, Vec<usize>>,
    by_ingredient: HashMap<u16, Vec<usize>>,
}

impl RecipeRegistry {
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    #[must_use]
    pub fn by_display_id(&self, display_id: i32) -> Option<&RecipeEntry> {
        self.by_display_id
            .get(&display_id)
            .map(|index| &self.entries[*index])
    }

    #[must_use]
    pub fn by_id(&self, id: &str) -> Option<&RecipeEntry> {
        self.by_id.get(id).map(|index| &self.entries[*index])
    }

    /// Recipes whose result is this item.
    #[must_use]
    pub fn producing(&self, item_id: u16) -> &[usize] {
        self.by_result.get(&item_id).map_or(&[], Vec::as_slice)
    }

    /// Recipes that take this item as an ingredient.
    #[must_use]
    pub fn consuming(&self, item_id: u16) -> &[usize] {
        self.by_ingredient.get(&item_id).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn id_of(&self, index: usize) -> &str {
        &self.entries[index].id
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

static REGISTRY: OnceLock<RecipeRegistry> = OnceLock::new();

/// The lazily built static recipe index.
pub fn registry() -> &'static RecipeRegistry {
    REGISTRY.get_or_init(build_registry)
}

fn item_id_of(name: &str) -> Option<u16> {
    let key = name.strip_prefix("minecraft:").unwrap_or(name);
    Item::from_registry_key(key).map(|item| item.id)
}

fn ingredient_item_ids(ingredient: &RecipeIngredientTypes, out: &mut Vec<u16>) {
    match ingredient {
        RecipeIngredientTypes::Simple(name) => out.extend(item_id_of(name)),
        RecipeIngredientTypes::OneOf(names) => {
            out.extend(names.iter().filter_map(|name| item_id_of(name)));
        }
        RecipeIngredientTypes::Tagged(tag) => {
            let tag = tag.strip_prefix('#').unwrap_or(tag);
            if let Some(ids) = get_tag_ids(RegistryKey::Item, tag) {
                out.extend_from_slice(ids);
            }
        }
    }
}

/// Resolves one `RECIPES_CRAFTING` entry's book-registration result, appending its ingredient
/// item ids into `ingredients`. Returns `None` for the two special-recipe kinds, which vanilla's
/// `ServerRecipeBook` never assigns a display id to (`ServerRecipeBook.java:66`).
fn crafting_recipe_book_result<'a>(
    recipe: &'a CraftingRecipeTypes,
    ingredients: &mut Vec<u16>,
) -> Option<&'a RecipeResultStruct> {
    match recipe {
        CraftingRecipeTypes::CraftingShaped { key, result, .. } => {
            for (_, ingredient) in *key {
                ingredient_item_ids(ingredient, ingredients);
            }
            Some(result)
        }
        CraftingRecipeTypes::CraftingShapeless {
            ingredients: list,
            result,
            ..
        } => {
            for ingredient in *list {
                ingredient_item_ids(ingredient, ingredients);
            }
            Some(result)
        }
        CraftingRecipeTypes::CraftingTransmute {
            input,
            material,
            result,
            ..
        } => {
            ingredient_item_ids(input, ingredients);
            ingredient_item_ids(material, ingredients);
            Some(result)
        }
        CraftingRecipeTypes::CraftingDye {
            target,
            dye,
            result,
            ..
        } => {
            ingredient_item_ids(target, ingredients);
            ingredient_item_ids(dye, ingredients);
            Some(result)
        }
        CraftingRecipeTypes::CraftingImbue {
            source,
            material,
            result,
            ..
        } => {
            ingredient_item_ids(source, ingredients);
            ingredient_item_ids(material, ingredients);
            Some(result)
        }
        CraftingRecipeTypes::CraftingDecoratedPot { .. } | CraftingRecipeTypes::CraftingSpecial => {
            None
        }
    }
}

fn build_registry() -> RecipeRegistry {
    let mut registry = RecipeRegistry::default();
    let mut ingredients = Vec::new();
    // Display ids are assigned exactly as `CRecipeBookAdd` writes them: crafting
    // recipes first, skipping the two special kinds, then cooking recipes.
    let mut display_id: i32 = 0;
    let mut used_ids: HashMap<String, u32> = HashMap::new();

    let mut push = |registry: &mut RecipeRegistry,
                    raw_id: &str,
                    display_id: i32,
                    result: Option<u16>,
                    ingredients: &[u16]| {
        // Crafting recipes have no id in the generated table, so several recipes for
        // the same result would collide; the first keeps the plain vanilla-shaped id
        // and the rest are suffixed, mirroring vanilla's own `_from_*` id convention.
        let count = used_ids.entry(raw_id.to_owned()).or_insert(0);
        *count += 1;
        let id = if *count == 1 {
            raw_id.to_owned()
        } else {
            format!("{raw_id}_alt{}", *count)
        };
        let index = registry.entries.len();
        if let Some(result) = result {
            registry.by_result.entry(result).or_default().push(index);
        }
        for item in ingredients {
            let bucket = registry.by_ingredient.entry(*item).or_default();
            if bucket.last() != Some(&index) {
                bucket.push(index);
            }
        }
        registry.by_id.insert(id.clone(), index);
        registry.by_display_id.insert(display_id, index);
        registry.entries.push(RecipeEntry {
            id,
            display_id,
            result: result.unwrap_or_default(),
        });
    };

    for recipe in RECIPES_CRAFTING {
        ingredients.clear();
        let Some(result) = crafting_recipe_book_result(recipe, &mut ingredients) else {
            continue;
        };
        ingredients.sort_unstable();
        ingredients.dedup();
        push(
            &mut registry,
            result.id,
            display_id,
            item_id_of(result.id),
            &ingredients,
        );
        display_id += 1;
    }

    for recipe in RECIPES_COOKING {
        let cooking = match recipe {
            CookingRecipeType::Smelting(r)
            | CookingRecipeType::Blasting(r)
            | CookingRecipeType::Smoking(r)
            | CookingRecipeType::CampfireCooking(r) => r,
        };
        ingredients.clear();
        ingredient_item_ids(&cooking.ingredient, &mut ingredients);
        ingredients.sort_unstable();
        ingredients.dedup();
        push(
            &mut registry,
            cooking.recipe_id,
            display_id,
            item_id_of(cooking.result.id),
            &ingredients,
        );
        display_id += 1;
    }

    // `StonecutterRecipe.display()` and `recipeBookCategory()` add these entries after the
    // cooking displays (`StonecutterRecipe.java:37-49`), matching the packet writer's order.
    for recipe in RECIPES_STONECUTTING {
        ingredients.clear();
        ingredient_item_ids(&recipe.ingredient, &mut ingredients);
        ingredients.sort_unstable();
        ingredients.dedup();
        push(
            &mut registry,
            recipe.result.id,
            display_id,
            item_id_of(recipe.result.id),
            &ingredients,
        );
        display_id += 1;
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_nbt() {
        let mut book = ServerRecipeBook::new();
        book.settings_mut()
            .set_book_setting(RecipeBookType::Crafting, true, false);
        book.settings_mut()
            .set_book_setting(RecipeBookType::Smoker, false, true);
        book.add("minecraft:oak_planks");

        let mut nbt = NbtCompound::new();
        book.write_nbt(&mut nbt);

        let mut loaded = ServerRecipeBook::new();
        loaded.read_nbt(&nbt);

        assert!(loaded.settings().is_open(RecipeBookType::Crafting));
        assert!(!loaded.settings().is_filtering(RecipeBookType::Crafting));
        assert!(!loaded.settings().is_open(RecipeBookType::Smoker));
        assert!(loaded.settings().is_filtering(RecipeBookType::Smoker));
        assert!(loaded.contains("minecraft:oak_planks"));
    }

    #[test]
    fn unknown_ids_are_dropped_on_load() {
        let mut nbt = NbtCompound::new();
        let mut book = NbtCompound::new();
        book.put_list(
            KNOWN_TAG,
            vec![NbtTag::String("minecraft:definitely_not_a_recipe".into())],
        );
        book.put_list(HIGHLIGHT_TAG, Vec::new());
        nbt.put_compound(RECIPE_BOOK_TAG, book);

        let mut loaded = ServerRecipeBook::new();
        loaded.read_nbt(&nbt);
        assert!(loaded.known().is_empty());
    }

    #[test]
    fn add_recipes_reports_only_new_unlocks() {
        let mut book = ServerRecipeBook::new();
        assert_eq!(book.add_recipes(["a", "b"]).len(), 2);
        assert_eq!(book.add_recipes(["a", "c"]), vec!["c".to_owned()]);
        assert!(book.is_highlighted("a"));
        book.remove_highlight("a");
        assert!(!book.is_highlighted("a"));
        assert!(book.contains("a"));
        assert_eq!(book.remove_recipes(["a", "zz"]), vec!["a".to_owned()]);
        assert!(!book.contains("a"));
    }

    #[test]
    fn registry_indexes_crafting_and_cooking() {
        let registry = registry();
        assert!(!registry.is_empty());
        // Every display id maps back to the entry that carries it.
        for index in 0..registry.len() {
            let entry = &registry.entries[index];
            assert_eq!(
                registry
                    .by_display_id(entry.display_id)
                    .map(|e| e.id.clone()),
                Some(entry.id.clone())
            );
        }
        let iron_ingot = Item::from_registry_key("iron_ingot").expect("iron ingot exists");
        assert!(!registry.producing(iron_ingot.id).is_empty());
        assert!(!registry.consuming(iron_ingot.id).is_empty());
    }
}
