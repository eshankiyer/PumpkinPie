use crate::{
    BoxedRegistry, ImmutableRegistry, Registry,
    builder::RegistryBuilder,
    error::{RegistryInitError, RegistryInsertError},
    value::{ErasedRegistryRef, LockedIterator, RegistryRef},
};
use pumpkin_util::identifier::Identifier;
use std::any::{Any, TypeId, type_name};
use tokio::sync::{RwLock, RwLockReadGuard};
pub struct MutableRegistry<T: Send + Sync + 'static>(pub(crate) RwLock<RegistryBuilder<T>>);

impl<T: Send + Sync + 'static> MutableRegistry<T> {
    pub fn new(
        static_entries: &'static [T],
        static_identifiers: &[Identifier],
    ) -> Result<Self, RegistryInitError> {
        Ok(Self(RwLock::new(RegistryBuilder::new(
            static_entries,
            static_identifiers,
        )?)))
    }

    pub fn register(&self, identifier: Identifier, value: T) -> Result<(), RegistryInsertError> {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .register(identifier, value)
    }

    #[must_use]
    pub fn get(&self, identifier: &Identifier) -> Option<RegistryRef<'_, T>> {
        RwLockReadGuard::try_map(
            self.0
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            |registry| registry.get(identifier),
        )
        .map(RegistryRef::Locked)
        .ok()
    }

    #[must_use]
    pub fn get_by_id(&self, id: usize) -> Option<RegistryRef<'_, T>> {
        RwLockReadGuard::try_map(
            self.0
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            |registry| registry.get_by_id(id),
        )
        .map(RegistryRef::Locked)
        .ok()
    }

    #[must_use]
    pub fn get_id(&self, identifier: &Identifier) -> Option<usize> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_id(identifier)
    }

    #[must_use]
    pub fn contains(&self, identifier: &Identifier) -> bool {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(identifier)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    #[allow(clippy::iter_not_returning_iterator)] // does clippy know how async works?
    pub fn iter(&self) -> impl Iterator<Item = (&Identifier, &T)> {
        LockedIterator::new(
            self.0
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl MutableRegistry<BoxedRegistry> {
    fn into_nested_immutable(self) -> ImmutableRegistry<BoxedRegistry> {
        let RegistryBuilder {
            static_entries,
            entries,
            mapping,
        } = self.0.into_inner();

        let mut immutable_entries = Vec::with_capacity(entries.len());

        for entry in entries {
            immutable_entries.push(entry.into_immutable());
        }

        ImmutableRegistry::new(
            static_entries,
            immutable_entries.into_boxed_slice(),
            mapping,
        )
    }
}

impl<T: Send + Sync + 'static> Registry for MutableRegistry<T> {
    fn item_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn item_type_name(&self) -> &'static str {
        type_name::<T>()
    }

    fn get_id(&self, identifier: &Identifier) -> Option<usize> {
        Self::get_id(self, identifier)
    }

    fn get_by_id(&self, id: usize) -> Option<ErasedRegistryRef<'_>> {
        Self::get_by_id(self, id).map(ErasedRegistryRef::new)
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }

    #[allow(clippy::expect_used)]
    fn into_immutable(self: Box<Self>) -> BoxedRegistry {
        let erased: Box<dyn Any + Send> = self;

        match erased.downcast::<MutableRegistry<BoxedRegistry>>() {
            Ok(registry) => Box::new(registry.into_nested_immutable()) as BoxedRegistry,
            Err(erased) => {
                let registry = erased
                    .downcast::<Self>()
                    .expect("downcast back to MutableRegistry<T> must succeed");

                Box::new(ImmutableRegistry::from(*registry)) as BoxedRegistry
            }
        }
    }
}
