use crate::value::ErasedRegistryRef;
use pumpkin_util::identifier::Identifier;
use std::any::{Any, TypeId};
use std::pin::Pin;

mod builder;
mod immutable;
mod mutable;

mod access;
mod key;
mod value;

pub mod error;
pub use crate::access::{RootRegistryOwner, RootRegistryReference, RootRegistryState};
pub use crate::immutable::ImmutableRegistry;
pub use crate::key::{ArcDataKey, DataKey, DataKeyBuilder, RefDataKey};
pub use crate::mutable::MutableRegistry;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Registry: Any + Send + Sync {
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    fn into_immutable(self: Box<Self>) -> BoxedRegistry;

    fn item_type_id(&self) -> TypeId;
    fn item_type_name(&self) -> &'static str;

    fn get_id(&self, identifier: &Identifier) -> Option<usize>;
    fn get_by_id(&self, id: usize) -> Option<ErasedRegistryRef<'_>>;
}

pub type BoxedRegistry = Box<dyn Registry>;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::error::{DataKeyBuildError, DataKeyGetError, RegistryInsertError};
    use std::sync::Arc;

    fn id(value: &'static str) -> Identifier {
        Identifier::parse_static(value)
    }

    fn nested_root() -> (RootRegistryOwner, RootRegistryReference) {
        let numbers = MutableRegistry::new(&[], &[]).unwrap();
        numbers.register(id("test:one"), 1u32).unwrap();
        numbers.register(id("test:two"), 2u32).unwrap();

        let owner = RootRegistryOwner::new(&[], &[]).unwrap();
        let root = owner.get();
        root.register(id("test:numbers"), Box::new(numbers))
            .unwrap();
        (owner, root)
    }

    #[tokio::test]
    fn mutable_registry_supports_lookup_and_guarded_iteration() {
        let registry = MutableRegistry::new(&[], &[]).unwrap();
        let first = id("test:first");
        let second = id("test:second");
        registry.register(first.clone(), 10u32).unwrap();
        registry.register(second.clone(), 20u32).unwrap();

        assert_eq!(registry.len(), 2);
        assert!(registry.contains(&first));
        assert_eq!(*registry.get(&first).unwrap(), 10);
        assert_eq!(*registry.get_by_id(1).unwrap(), 20);

        let mut values = registry.iter().map(|(_, value)| *value).collect::<Vec<_>>();
        values.sort_unstable();
        assert_eq!(values, vec![10, 20]);
    }

    #[tokio::test]
    fn root_lock_recursively_freezes_dynamic_child_registries() {
        let (owner, root) = nested_root();
        assert!(!root.is_locked());

        owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        assert!(root.is_locked());
        assert_eq!(root.len(), 1);
        assert!(matches!(
            root.register(
                id("test:other"),
                Box::new(MutableRegistry::<u32>::new(&[], &[]).unwrap())
            ),
            Err(RegistryInsertError::Immutable)
        ));

        let child = root.get(&id("test:numbers")).unwrap();
        assert_eq!(child.item_type_id(), TypeId::of::<u32>());
        let value = child.get_by_id(1).unwrap();
        assert_eq!(value.downcast_ref::<u32>(), Some(&2));
    }

    #[tokio::test]
    fn root_lock_is_idempotent_and_keeps_ids_stable() {
        let (owner, root) = nested_root();
        let before = root.get_id(&id("test:numbers"));

        owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        assert_eq!(before, Some(0));
        assert_eq!(root.get_id(&id("test:numbers")), before);
        assert!(root.get_by_id(0).is_some());
    }

    #[tokio::test]
    fn ref_data_key_resolves_nested_values_before_and_after_locking() {
        let (owner, root) = nested_root();
        let key = DataKeyBuilder::<u32>::new()
            .child(id("test:numbers"))
            .child(id("test:two"))
            .build_ref(&*root)
            .unwrap();

        assert_eq!(key.ids(), &[0, 1]);
        assert_eq!(key.with(|value| *value).unwrap(), 2);

        owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(key.with(|value| *value).unwrap(), 2);
    }

    #[tokio::test]
    fn arc_data_key_keeps_the_registry_tree_alive() {
        let (owner, root) = nested_root();
        let root: Arc<dyn Registry> = root;
        let key = DataKeyBuilder::<u32>::new()
            .child(id("test:numbers"))
            .child(id("test:one"))
            .build_arc(&root)
            .unwrap();
        drop(root);
        drop(owner);

        assert_eq!(key.ids(), &[0, 0]);
        assert_eq!(key.with(|value| *value).unwrap(), 1);
    }

    #[tokio::test]
    fn data_key_builder_reports_structural_errors() {
        let (_owner, root) = nested_root();

        assert!(matches!(
            DataKeyBuilder::<u32>::new().build_ref(&*root),
            Err(DataKeyBuildError::Empty)
        ));

        let missing_registry = id("test:missing_registry");
        assert!(matches!(
            DataKeyBuilder::<u32>::new()
                .child(missing_registry.clone())
                .child(id("test:value"))
                .build_ref(&*root),
            Err(DataKeyBuildError::MissingRegistry(found)) if found == missing_registry
        ));

        let missing_value = id("test:missing_value");
        assert!(matches!(
            DataKeyBuilder::<u32>::new()
                .child(id("test:numbers"))
                .child(missing_value.clone())
                .build_ref(&*root),
            Err(DataKeyBuildError::MissingValue(found)) if found == missing_value
        ));
    }

    #[tokio::test]
    fn data_key_get_reports_value_type_mismatch() {
        let (_owner, root) = nested_root();
        let key = DataKeyBuilder::<u64>::new()
            .child(id("test:numbers"))
            .child(id("test:one"))
            .build_ref(&*root)
            .unwrap();

        assert!(matches!(
            key.with(|value| *value),
            Err(DataKeyGetError::TypeMismatch { .. })
        ));
    }
}
