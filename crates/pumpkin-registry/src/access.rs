use crate::{
    BoxedRegistry, ImmutableRegistry, Registry,
    builder::RegistryBuilder,
    error::{RegistryInitError, RegistryInsertError},
    value::{DynIterator, ErasedRegistryRef, LockedIterator, RegistryRef},
};
use pumpkin_util::identifier::Identifier;
use std::{
    any::{TypeId, type_name},
    sync::{Arc, OnceLock},
};
use tokio::sync::{RwLock, RwLockReadGuard};

pub struct RootRegistryState {
    mutable: RwLock<Option<RegistryBuilder<BoxedRegistry>>>,
    immutable: OnceLock<ImmutableRegistry<BoxedRegistry>>,
}

pub struct RootRegistryOwner {
    state: Arc<RootRegistryState>,
}

pub type RootRegistryReference = Arc<RootRegistryState>;

impl RootRegistryOwner {
    pub fn new(
        static_entries: &'static [BoxedRegistry],
        static_identifiers: &[Identifier],
    ) -> Result<Self, RegistryInitError> {
        let state = Arc::new(RootRegistryState {
            mutable: RwLock::new(Some(RegistryBuilder::new(
                static_entries,
                static_identifiers,
            )?)),
            immutable: OnceLock::new(),
        });

        Ok(Self {
            state: Arc::clone(&state),
        })
    }

    #[must_use]
    pub fn get(&self) -> RootRegistryReference {
        self.state.clone()
    }

    #[allow(clippy::panic)]
    pub fn lock(&self) {
        let Some(builder) = self
            .state
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };

        let RegistryBuilder {
            static_entries,
            entries,
            mapping,
        } = builder;

        let mut immutable_entries = Vec::with_capacity(entries.len());

        for entry in entries {
            immutable_entries.push(entry.into_immutable());
        }

        if self
            .state
            .immutable
            .set(ImmutableRegistry::new(
                static_entries,
                immutable_entries.into_boxed_slice(),
                mapping,
            ))
            .is_err()
        {
            panic!("RootRegistry internal state was altered externally")
        }
    }
}

impl RootRegistryState {
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.immutable.get().is_some()
    }

    pub fn register(
        &self,
        identifier: Identifier,
        value: BoxedRegistry,
    ) -> Result<(), RegistryInsertError> {
        if self.is_locked() {
            return Err(RegistryInsertError::Immutable);
        }

        let mut mutable = self
            .mutable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        mutable
            .as_mut()
            .ok_or(RegistryInsertError::Immutable)?
            .register(identifier, value)
    }

    #[must_use]
    pub fn get(&self, identifier: &Identifier) -> Option<RegistryRef<'_, BoxedRegistry>> {
        if let Some(registry) = self.immutable.get() {
            return registry.get(identifier).map(RegistryRef::Borrowed);
        }

        RwLockReadGuard::try_map(
            self.mutable
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            |registry| registry.as_ref()?.get(identifier),
        )
        .map(RegistryRef::Locked)
        .ok()
    }

    #[must_use]
    pub fn get_by_id(&self, id: usize) -> Option<RegistryRef<'_, BoxedRegistry>> {
        if let Some(registry) = self.immutable.get() {
            return registry.get_by_id(id).map(RegistryRef::Borrowed);
        }

        RwLockReadGuard::try_map(
            self.mutable
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            |registry| registry.as_ref()?.get_by_id(id),
        )
        .map(RegistryRef::Locked)
        .ok()
    }

    #[must_use]
    pub fn get_id(&self, identifier: &Identifier) -> Option<usize> {
        if let Some(registry) = self.immutable.get() {
            return registry.get_id(identifier);
        }

        let mutable = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mutable.as_ref()?.get_id(identifier)
    }

    #[must_use]
    pub fn contains(&self, identifier: &Identifier) -> bool {
        self.get_id(identifier).is_some()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        if let Some(registry) = self.immutable.get() {
            return registry.len();
        }

        self.mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(0, RegistryBuilder::len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[allow(clippy::iter_not_returning_iterator)]
    pub fn iter(&self) -> impl Iterator<Item = (&Identifier, &BoxedRegistry)> {
        if let Some(registry) = self.immutable.get() {
            return DynIterator::new(registry.iter());
        }

        let guard = self
            .mutable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        RwLockReadGuard::try_map(guard, Option::as_ref).map_or_else(
            |_| DynIterator::new(std::iter::empty()),
            |guard| DynIterator::new(LockedIterator::new(guard)),
        )
    }
}

impl Registry for RootRegistryState {
    fn item_type_id(&self) -> TypeId {
        TypeId::of::<BoxedRegistry>()
    }

    fn item_type_name(&self) -> &'static str {
        type_name::<BoxedRegistry>()
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

    fn into_immutable(self: Box<Self>) -> BoxedRegistry {
        self as BoxedRegistry
    }
}
