use crate::{
    BoxedRegistry, Registry,
    error::{DataKeyBuildError, DataKeyGetError},
};
use pumpkin_util::identifier::Identifier;
use std::{any::type_name, marker::PhantomData, sync::Arc};

pub trait DataKey<T>
where
    T: Send + Sync + 'static,
{
    /// Returns the numeric path, including the final value ID.
    fn ids(&self) -> &[usize];

    /// Returns the root registry this key belongs to.
    fn root_registry(&self) -> &dyn Registry;

    /// Runs `callback` while all registry guards needed to access the value
    /// remain alive.
    fn with<'a, V, F>(&'a self, callback: F) -> Result<V, DataKeyGetError>
    where
        V: Send + 'a,
        F: FnOnce(&T) -> V + Send + 'a,
    {
        with_from_key(self.root_registry(), self.ids(), callback)
    }
}

fn with_from_key<'a, T, V, F>(
    root: &'a dyn Registry,
    keys: &'a [usize],
    callback: F,
) -> Result<V, DataKeyGetError>
where
    T: Send + Sync + 'static,
    V: Send + 'a,
    F: FnOnce(&T) -> V + Send + 'a,
{
    let Some((&value_id, registry_path)) = keys.split_last() else {
        return Err(DataKeyGetError::InvalidKey);
    };

    with_from_registry(root, registry_path, value_id, callback)
}

fn with_from_registry<'a, T, V, F>(
    current: &'a dyn Registry,
    registry_path: &'a [usize],
    value_id: usize,
    callback: F,
) -> Result<V, DataKeyGetError>
where
    T: Send + Sync + 'static,
    V: Send + 'a,
    F: FnOnce(&T) -> V + Send + 'a,
{
    let Some((&registry_id, remaining_path)) = registry_path.split_first() else {
        let value = current
            .get_by_id(value_id)
            .ok_or(DataKeyGetError::MissingValue { id: value_id })?;

        let value = value
            .downcast_ref::<T>()
            .ok_or(DataKeyGetError::TypeMismatch {
                expected: type_name::<T>(),
                actual: current.item_type_name(),
            })?;

        return Ok(callback(value));
    };

    let registry = current
        .get_by_id(registry_id)
        .ok_or(DataKeyGetError::MissingRegistry { id: registry_id })?;

    let registry = registry
        .downcast_ref::<BoxedRegistry>()
        .ok_or(DataKeyGetError::MissingRegistry { id: registry_id })?;

    // `registry` borrows from `value`, so `value` and its lock guard
    // remain alive throughout the recursive call.
    with_from_registry(registry.as_ref(), remaining_path, value_id, callback)
}
pub struct ArcDataKey<T: Send + Sync + 'static> {
    keys: Box<[usize]>,
    root: Arc<dyn Registry>,
    marker: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> Clone for ArcDataKey<T> {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            root: Arc::clone(&self.root),
            marker: PhantomData,
        }
    }
}

impl<T: Send + Sync + 'static> DataKey<T> for ArcDataKey<T> {
    fn ids(&self) -> &[usize] {
        &self.keys
    }

    fn root_registry(&self) -> &dyn Registry {
        self.root.as_ref()
    }
}

pub struct RefDataKey<'a, T: Send + Sync + 'static> {
    keys: Box<[usize]>,
    root: &'a dyn Registry,
    marker: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> Clone for RefDataKey<'_, T> {
    fn clone(&self) -> Self {
        Self {
            keys: self.keys.clone(),
            root: self.root,
            marker: PhantomData,
        }
    }
}

impl<T: Send + Sync + 'static> DataKey<T> for RefDataKey<'_, T> {
    fn ids(&self) -> &[usize] {
        &self.keys
    }

    fn root_registry(&self) -> &dyn Registry {
        self.root
    }
}

pub struct DataKeyBuilder<T: Send + Sync + 'static> {
    keys: Vec<Identifier>,
    marker: PhantomData<fn() -> T>,
}

impl<T: Send + Sync + 'static> DataKeyBuilder<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: Vec::new(),
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn child(mut self, identifier: Identifier) -> Self {
        self.keys.push(identifier);
        self
    }

    fn build_keys(&self, registry: &dyn Registry) -> Result<Box<[usize]>, DataKeyBuildError> {
        let Some((value_identifier, registry_path)) = self.keys.split_last() else {
            return Err(DataKeyBuildError::Empty);
        };

        let mut numeric_keys = Vec::with_capacity(self.keys.len());

        build_key_path::<T>(registry, registry_path, value_identifier, &mut numeric_keys)?;

        Ok(numeric_keys.into_boxed_slice())
    }

    pub fn build_arc(
        self,
        registry: &Arc<dyn Registry>,
    ) -> Result<ArcDataKey<T>, DataKeyBuildError> {
        let keys = self.build_keys(registry.as_ref())?;

        Ok(ArcDataKey {
            keys,
            root: Arc::clone(registry),
            marker: PhantomData,
        })
    }

    pub fn build_ref(
        self,
        registry: &dyn Registry,
    ) -> Result<RefDataKey<'_, T>, DataKeyBuildError> {
        let keys = self.build_keys(registry)?;

        Ok(RefDataKey {
            keys,
            root: registry,
            marker: PhantomData,
        })
    }
}

impl<T: Send + Sync + 'static> Default for DataKeyBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

fn build_key_path<'a, T>(
    current: &'a dyn Registry,
    registry_path: &'a [Identifier],
    value_identifier: &'a Identifier,
    numeric_keys: &'a mut Vec<usize>,
) -> Result<(), DataKeyBuildError>
where
    T: Send + Sync + 'static,
{
    let Some((identifier, remaining_path)) = registry_path.split_first() else {
        let value_id = current
            .get_id(value_identifier)
            .ok_or_else(|| DataKeyBuildError::MissingValue(value_identifier.clone()))?;

        numeric_keys.push(value_id);
        return Ok(());
    };

    let id = current
        .get_id(identifier)
        .ok_or_else(|| DataKeyBuildError::MissingRegistry(identifier.clone()))?;

    let registry = current
        .get_by_id(id)
        .ok_or_else(|| DataKeyBuildError::MissingRegistry(identifier.clone()))?;

    let registry = registry
        .downcast_ref::<BoxedRegistry>()
        .ok_or_else(|| DataKeyBuildError::NotARegistry(identifier.clone()))?;

    numeric_keys.push(id);

    build_key_path::<T>(
        registry.as_ref(),
        remaining_path,
        value_identifier,
        numeric_keys,
    )
}
