use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, Copy)]
#[repr(i8)]
pub enum ModifierOperation {
    Add = 0,           // add value
    MultiplyBase = 1,  // multiply base (base * (1 + x))
    MultiplyTotal = 2, // multiply total (applied last)
}

#[derive(Clone, Debug)]
pub struct Modifier {
    pub id: String,
    pub amount: f64,
    pub operation: ModifierOperation,
}

/// Per-entity attribute instance used at runtime.
#[derive(Debug)]
pub struct AttributeInstance {
    pub base_value: f64,
    pub min_value: f64,
    pub max_value: f64,
    pub modifiers: Vec<Modifier>,
    /// Mirrors vanilla's permanent/transient modifier maps (`AttributeInstance.java:23-25`).
    permanent_modifier_ids: HashSet<String>,
    transient_modifier_ids: HashSet<String>,
    pub cached_value: AtomicU64,
    pub dirty: AtomicBool,
}

impl AttributeInstance {
    #[must_use]
    pub fn new(base_value: f64, min_value: f64, max_value: f64) -> Self {
        Self {
            base_value,
            min_value,
            max_value,
            modifiers: Vec::new(),
            permanent_modifier_ids: HashSet::new(),
            transient_modifier_ids: HashSet::new(),
            cached_value: AtomicU64::new(base_value.to_bits()),
            dirty: AtomicBool::new(true),
        }
    }

    pub fn value(&self) -> f64 {
        if !self.dirty.load(Ordering::Relaxed) {
            return f64::from_bits(self.cached_value.load(Ordering::Relaxed));
        }

        let mut value = self.base_value;

        let mut add_sum = 0.0;
        let mut mul_total = 1.0;
        for m in &self.modifiers {
            match m.operation {
                ModifierOperation::Add => add_sum += m.amount,
                ModifierOperation::MultiplyBase => {}
                ModifierOperation::MultiplyTotal => mul_total *= 1.0 + m.amount,
            }
        }

        value += add_sum;
        let base = value;
        for modifier in self
            .modifiers
            .iter()
            .filter(|modifier| matches!(modifier.operation, ModifierOperation::MultiplyBase))
        {
            value += base * modifier.amount;
        }
        value *= mul_total;

        value = sanitize_value(value, self.min_value, self.max_value);

        self.cached_value.store(value.to_bits(), Ordering::Relaxed);
        self.dirty.store(false, Ordering::Relaxed);

        value
    }

    /// Vanilla `AttributeInstance.setBaseValue` (`AttributeInstance.java:45-49`): changing the
    /// base invalidates the cached value, while assigning the same value leaves the cache state
    /// untouched.
    pub fn set_base_value(&mut self, base_value: f64) {
        if base_value != self.base_value {
            self.base_value = base_value;
            self.set_dirty();
        }
    }

    /// Vanilla `AttributeInstance.setDirty` (`AttributeInstance.java:112-115`).
    pub fn set_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn add_or_replace_modifier(&mut self, modifier: Modifier) {
        if let Some(pos) = self.modifiers.iter().position(|m| m.id == modifier.id) {
            self.modifiers.remove(pos);
        }
        self.modifiers.push(modifier);
        self.set_dirty();
    }

    /// Vanilla `AttributeInstance.addOrUpdateTransientModifier` and
    /// `addOrReplacePermanentModifier` (`AttributeInstance.java:83-99`). The Rust model keeps
    /// modifier lifetime in explicit ID sets, while both operations share the same ID-replacing
    /// storage primitive.
    pub fn add_or_update_transient_modifier(&mut self, modifier: Modifier) {
        self.permanent_modifier_ids.remove(&modifier.id);
        self.transient_modifier_ids.insert(modifier.id.clone());
        self.add_or_replace_modifier(modifier);
    }

    pub fn add_or_replace_permanent_modifier(&mut self, modifier: Modifier) {
        self.transient_modifier_ids.remove(&modifier.id);
        self.permanent_modifier_ids.insert(modifier.id.clone());
        self.add_or_replace_modifier(modifier);
    }

    /// Vanilla `AttributeInstance.addTransientModifier` and `addPermanentModifier`
    /// (`AttributeInstance.java:91-104`) reject a duplicate ID. Returning `false` preserves that
    /// result without panicking a server command or plugin caller.
    pub fn add_transient_modifier(&mut self, modifier: Modifier) -> bool {
        if self.has_modifier(&modifier.id) {
            return false;
        }
        self.permanent_modifier_ids.remove(&modifier.id);
        self.transient_modifier_ids.insert(modifier.id.clone());
        self.modifiers.push(modifier);
        self.set_dirty();
        true
    }

    pub fn add_permanent_modifier(&mut self, modifier: Modifier) -> bool {
        if self.has_modifier(&modifier.id) {
            return false;
        }
        self.transient_modifier_ids.remove(&modifier.id);
        self.permanent_modifier_ids.insert(modifier.id.clone());
        self.modifiers.push(modifier);
        self.set_dirty();
        true
    }

    /// Vanilla `AttributeInstance.addPermanentModifiers` (`AttributeInstance.java:106-110`).
    pub fn add_permanent_modifiers<I>(&mut self, modifiers: I)
    where
        I: IntoIterator<Item = Modifier>,
    {
        for modifier in modifiers {
            let _ = self.add_permanent_modifier(modifier);
        }
    }

    /// Vanilla `AttributeInstance.hasModifier` (`AttributeInstance.java:65-71`).
    #[must_use]
    pub fn has_modifier(&self, id: &str) -> bool {
        self.modifiers.iter().any(|modifier| modifier.id == id)
    }

    /// Vanilla `AttributeInstance.getModifiers` and `getPermanentModifiers`
    /// (`AttributeInstance.java:57-63`), represented by the live modifier vector and its
    /// explicit lifetime set.
    #[must_use]
    pub fn get_modifiers(&self) -> &[Modifier] {
        &self.modifiers
    }

    #[must_use]
    pub fn get_permanent_modifiers(&self) -> Vec<&Modifier> {
        self.modifiers
            .iter()
            .filter(|modifier| self.is_permanent_modifier(&modifier.id))
            .collect()
    }

    pub(crate) fn is_permanent_modifier(&self, id: &str) -> bool {
        self.permanent_modifier_ids.contains(id)
            || (!self.transient_modifier_ids.contains(id) && legacy_permanent_modifier(id))
    }

    pub fn remove_modifier(&mut self, id: &str) {
        if let Some(pos) = self.modifiers.iter().position(|m| m.id == id) {
            self.modifiers.swap_remove(pos);
        }
        self.permanent_modifier_ids.remove(id);
        self.transient_modifier_ids.remove(id);
        self.set_dirty();
    }

    /// Vanilla `AttributeInstance.removeModifiers` (`AttributeInstance.java:133-136`).
    pub fn remove_modifiers(&mut self) {
        if !self.modifiers.is_empty() {
            self.modifiers.clear();
            self.permanent_modifier_ids.clear();
            self.transient_modifier_ids.clear();
            self.set_dirty();
        }
    }

    /// Vanilla `AttributeInstance.replaceFrom` (`AttributeInstance.java:172-184`). The cached
    /// value is deliberately not copied: vanilla marks the destination dirty and recalculates it
    /// on the next read.
    pub fn replace_from(&mut self, other: &Self) {
        self.base_value = other.base_value;
        self.modifiers.clone_from(&other.modifiers);
        self.permanent_modifier_ids
            .clone_from(&other.permanent_modifier_ids);
        self.transient_modifier_ids
            .clone_from(&other.transient_modifier_ids);
        self.set_dirty();
    }
}

/// Send updates for multiple attributes in a single packet for the given living entity.
pub async fn send_attribute_updates_for_living(
    living: &crate::entity::living::LivingEntity,
    attributes: Vec<Attributes>,
) {
    use pumpkin_protocol::bedrock::client::update_attributes::{
        Attribute as BeAttribute, CUpdateAttributes as BePacket,
    };
    use pumpkin_protocol::codec::var_int::VarInt;
    use pumpkin_protocol::codec::{var_uint::VarUInt, var_ulong::VarULong};
    use pumpkin_protocol::java::client::play::AttributeModifier as JeAttrMod;
    use pumpkin_protocol::java::client::play::CUpdateAttributes as JePacket;
    use pumpkin_protocol::java::client::play::Property as JeProperty;

    let mut je_properties: Vec<JeProperty> = Vec::with_capacity(attributes.len());
    let mut be_attributes: Vec<BeAttribute> = Vec::with_capacity(attributes.len());

    for attribute in attributes {
        let base_value = living.get_attribute_base(&attribute);
        let effective_value = living.get_attribute_value(&attribute);

        // Pull modifiers for this attribute
        let mut modifiers = Vec::new();
        if let Some(inst) = living
            .attributes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&attribute.id)
        {
            for mod_inst in &inst.modifiers {
                modifiers.push(JeAttrMod::new(
                    mod_inst.id.clone(),
                    mod_inst.amount,
                    mod_inst.operation as i8,
                ));
            }
        }

        // Move modifiers into the property
        je_properties.push(JeProperty::new(
            VarInt(i32::from(attribute.id)),
            base_value,
            modifiers,
        ));

        let name = match attribute.id {
            22 => "minecraft:movement".to_string(),
            19 => "minecraft:health".to_string(),
            18 => "minecraft:absorption".to_string(),
            2 => "minecraft:attack_damage".to_string(),
            0 => "minecraft:armor".to_string(),
            16 => "minecraft:knockback_resistance".to_string(),
            17 => "minecraft:luck".to_string(),
            13 => "minecraft:follow_range".to_string(),
            15 => "minecraft:horse.jump_strength".to_string(),
            // Fallback for others
            _ => format!("minecraft:attribute.{}", attribute.id),
        };

        let be_attribute = BeAttribute {
            min_value: 0.0,
            max_value: 3.402_823_5E38,
            current_value: effective_value as f32,
            default_min_value: 0.0,
            default_max_value: 3.402_823_5E38,
            default_value: base_value as f32,
            name,
            // Bedrock receives the already-computed effective value above. Do not advertise
            // modifier entries until their payload is encoded as well.
            modifiers_list_size: VarUInt(0),
        };

        be_attributes.push(be_attribute);
    }

    let je_packet = JePacket::new(living.entity.entity_id.into(), je_properties);

    let runtime_id = living.entity.entity_id as u64;
    let be_packet = BePacket {
        runtime_id: VarULong(runtime_id),
        attributes: be_attributes,
        player_tick: VarULong(0),
    };

    living
        .entity
        .world
        .load()
        .broadcast_editioned(&je_packet, &be_packet)
        .await;
}

impl Clone for AttributeInstance {
    fn clone(&self) -> Self {
        // Vanilla `AttributeInstance.replaceFrom` (`AttributeInstance.java:172-184`) copies both
        // modifier maps before invalidating the destination cache.
        Self {
            base_value: self.base_value,
            min_value: self.min_value,
            max_value: self.max_value,
            modifiers: self.modifiers.clone(),
            permanent_modifier_ids: self.permanent_modifier_ids.clone(),
            transient_modifier_ids: self.transient_modifier_ids.clone(),
            cached_value: AtomicU64::new(self.cached_value.load(Ordering::Relaxed)),
            dirty: AtomicBool::new(self.dirty.load(Ordering::Relaxed)),
        }
    }
}

/// Legacy callers predate the explicit lifetime sets; these IDs preserve their existing save
/// behavior from `AttributeInstance.pack` (`AttributeInstance.java:186-188`).
fn legacy_permanent_modifier(id: &str) -> bool {
    const TRANSIENT_PREFIXES: [&str; 1] = ["minecraft:enchantment."];
    const TRANSIENT_IDS: [&str; 3] = ["minecraft:attacking", "witch_drinking", "evil"];
    !TRANSIENT_PREFIXES
        .iter()
        .any(|prefix| id.starts_with(prefix))
        && !TRANSIENT_IDS.contains(&id)
}

pub(crate) const fn sanitize_value(value: f64, min_value: f64, max_value: f64) -> f64 {
    if value.is_nan() {
        min_value
    } else {
        value.clamp(min_value, max_value)
    }
}

#[cfg(test)]
mod tests {
    use super::{AttributeInstance, Modifier, ModifierOperation};

    #[test]
    fn ranged_values_match_vanilla_sanitization() {
        let mut instance = AttributeInstance::new(100.0, 0.0, 30.0);
        assert_eq!(instance.base_value, 100.0);
        assert_eq!(instance.value(), 30.0);

        instance.add_or_replace_modifier(Modifier {
            id: "over-max".to_string(),
            amount: 100.0,
            operation: ModifierOperation::Add,
        });
        assert_eq!(instance.value(), 30.0);

        let nan = AttributeInstance::new(f64::NAN, 0.0, 30.0);
        assert!(nan.base_value.is_nan());
        assert_eq!(nan.value(), 0.0);

        let negative = AttributeInstance::new(-1.0, -2.0, 1.0);
        assert_eq!(negative.value(), -1.0);

        let mut non_finite = AttributeInstance::new(f64::INFINITY, 0.0, 30.0);
        non_finite.add_or_replace_modifier(Modifier {
            id: "zero-multiplier".to_string(),
            amount: 0.0,
            operation: ModifierOperation::MultiplyBase,
        });
        assert_eq!(non_finite.value(), 0.0);
    }

    #[test]
    fn modifier_lifecycle_helpers_invalidate_and_replace() {
        // `AttributeInstance.java:45-49,83-110,112-115,133-136,172-184` requires every
        // successful mutation to invalidate the cached value and replacement to copy the live
        // base/modifier state.
        let mut instance = AttributeInstance::new(2.0, 0.0, 30.0);
        instance.add_permanent_modifiers([
            Modifier {
                id: "first".to_string(),
                amount: 3.0,
                operation: ModifierOperation::Add,
            },
            Modifier {
                id: "second".to_string(),
                amount: 2.0,
                operation: ModifierOperation::MultiplyTotal,
            },
        ]);
        assert_eq!(instance.value(), 15.0);
        assert_eq!(instance.get_modifiers().len(), 2);
        assert_eq!(instance.get_permanent_modifiers().len(), 2);
        assert!(!instance.add_transient_modifier(Modifier {
            id: "first".to_string(),
            amount: 9.0,
            operation: ModifierOperation::Add,
        }));

        instance.set_base_value(4.0);
        assert_eq!(instance.value(), 21.0);
        assert!(instance.has_modifier("first"));
        instance.remove_modifiers();
        assert_eq!(instance.value(), 4.0);

        let mut replacement = AttributeInstance::new(7.0, 0.0, 30.0);
        replacement.add_or_update_transient_modifier(Modifier {
            id: "replacement".to_string(),
            amount: 1.0,
            operation: ModifierOperation::MultiplyBase,
        });
        instance.replace_from(&replacement);
        assert_eq!(instance.base_value, 7.0);
        assert_eq!(instance.value(), 14.0);
        assert!(instance.has_modifier("replacement"));
        assert!(replacement.get_permanent_modifiers().is_empty());
    }
}

/// Registry storing per-entity-type base attribute overrides.
/// Internally stores a map from `entity_type.id` -> `HashMap`<attribute.id, f64> for O(1) lookup.
#[derive(Default)]
pub struct AttributeRegistry {
    map: HashMap<u16, HashMap<u8, f64>>,
}

impl AttributeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the base value for `attribute` for the given entity type id.
    /// If no override exists, returns `attribute.default_value`.
    #[must_use]
    pub fn get_base_value(&self, entity_type_id: u16, attribute: &Attributes) -> f64 {
        self.map
            .get(&entity_type_id)
            .and_then(|map| map.get(&attribute.id))
            .copied()
            .unwrap_or(attribute.default_value)
    }

    /// Return a vector of overrides for the given entity type id.
    /// This allows populating per-entity local attribute instances at spawn time.
    #[must_use]
    pub fn get_overrides_for_entity(&self, entity_type_id: u16) -> Option<Vec<(u8, f64)>> {
        self.map
            .get(&entity_type_id)
            .map(|m| m.iter().map(|(&k, &v)| (k, v)).collect())
    }
}

/// Builder to declaratively assemble attribute overrides for an entity type.
#[derive(Default)]
pub struct AttributeBuilder {
    entries: Vec<(Attributes, f64)>,
}

impl AttributeBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn add(mut self, attribute: Attributes, base: f64) -> Self {
        self.entries.push((attribute, base));
        self
    }

    #[must_use]
    pub fn build(self) -> Vec<(Attributes, f64)> {
        self.entries
    }
}

impl AttributeRegistry {
    /// Register overrides created by an `AttributeBuilder` for `entity_type`.
    pub fn register_builder(
        &mut self,
        entity_type: &'static EntityType,
        builder: AttributeBuilder,
    ) {
        let inner = self.map.entry(entity_type.id).or_default();
        for (attr, val) in builder.build() {
            inner.insert(attr.id, val);
        }
    }
}
