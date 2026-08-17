use std::{
    any::Any,
    sync::{Arc, LazyLock},
};

use libloading::Library;

use crate::plugin::{
    PLUGIN_API_VERSION,
    loader::{PluginLoadFuture, PluginUnloadFuture},
};

use super::{LoaderError, Path, Plugin, PluginLoader, PluginMetadata};

pub struct NativePluginLoader;

/// Upper bound for plausible metadata text field lengths, in bytes.
const MAX_METADATA_TEXT_LEN: usize = 4096;
/// Upper bound for plausible metadata list lengths, in entries.
const MAX_METADATA_LIST_LEN: usize = 1024;

/// Best-effort guard against implausible metadata from an ABI-incompatible
/// native plugin. Native plugins still need to be built against this Pumpkin
/// build because `PluginMetadata` is not a stable ABI representation.
fn is_metadata_plausible(metadata: &PluginMetadata) -> bool {
    let text_plausible =
        |text: &String| text.len() <= MAX_METADATA_TEXT_LEN && text.len() <= text.capacity();
    // The inner strings must be checked too: a list header can look sane
    // while its elements are garbage, and those elements are cloned as well.
    let list_plausible = |list: &Vec<String>| {
        list.len() <= MAX_METADATA_LIST_LEN
            && list.len() <= list.capacity()
            && list.iter().all(text_plausible)
    };

    !metadata.name.is_empty()
        && text_plausible(&metadata.name)
        && text_plausible(&metadata.version)
        && text_plausible(&metadata.description)
        && list_plausible(&metadata.authors)
        && list_plausible(&metadata.dependencies)
        && list_plausible(&metadata.permissions)
}

impl PluginLoader for NativePluginLoader {
    fn load<'a>(&'a self, path: &'a Path) -> PluginLoadFuture<'a> {
        Box::pin(async {
            let path = path.to_owned();

            // SAFETY: Loading dynamic library from path configured by server administrator.
            let library = unsafe { Library::new(&path) }
                .map_err(|e| LoaderError::LibraryLoad(e.to_string()))?;

            // Ensure this plugin was built against a compatible Pumpkin plugin API version
            // SAFETY: `PUMPKIN_API_VERSION` is an exported `u32` constant symbol created by `#[plugin_impl]`.
            let plugin_api_version = unsafe {
                match library.get::<*const u32>(b"PUMPKIN_API_VERSION") {
                    Ok(symbol) => **symbol,
                    Err(_) => return Err(LoaderError::ApiVersionMissing),
                }
            };

            if plugin_api_version != PLUGIN_API_VERSION {
                return Err(LoaderError::ApiVersionMismatch {
                    plugin_version: plugin_api_version,
                    server_version: PLUGIN_API_VERSION,
                });
            }

            // 2. Extract Metadata (METADATA)
            // `#[plugin_impl]` exports this as a `LazyLock`, since `PluginMetadata`
            // owns its strings and can't be built in a const.
            // SAFETY: `METADATA` is an exported `LazyLock<PluginMetadata>` symbol created by `#[plugin_impl]`.
            let metadata = unsafe {
                let metadata = library
                    .get::<*const LazyLock<PluginMetadata>>(b"METADATA")
                    .map_err(|_| LoaderError::MetadataMissing)?;
                let metadata: &PluginMetadata = &**metadata;
                if !is_metadata_plausible(metadata) {
                    return Err(LoaderError::MetadataCorrupt);
                }
                metadata.clone()
            };

            // 3. Extract Plugin Factory (plugin)
            // SAFETY: `plugin` is an exported constructor function symbol with signature `fn() -> Box<dyn Plugin>` created by `#[plugin_impl]`.
            let plugin_factory = unsafe {
                library
                    .get::<fn() -> Box<dyn Plugin>>(b"plugin")
                    .map_err(|_| LoaderError::EntrypointMissing)?
            };

            Ok((
                Arc::from(plugin_factory()),
                metadata,
                Box::new(library) as Box<dyn Any + Send + Sync>,
            ))
        })
    }

    fn can_load(&self, path: &Path) -> bool {
        let ext = path.extension().unwrap_or_default();

        if cfg!(target_os = "windows") {
            ext.eq_ignore_ascii_case("dll")
        } else if cfg!(target_os = "macos") {
            ext.eq_ignore_ascii_case("dylib")
        } else {
            ext.eq_ignore_ascii_case("so")
        }
    }

    fn unload(&self, data: Box<dyn Any + Send + Sync>) -> PluginUnloadFuture<'_> {
        Box::pin(async {
            data.downcast::<Library>()
                .map_or(Err(LoaderError::InvalidLoaderData), |library| {
                    drop(library);
                    Ok(())
                })
        })
    }

    /// Windows specific issue: Windows locks DLLs, so we must indicate they cannot be unloaded.
    fn can_unload(&self) -> bool {
        !cfg!(target_os = "windows")
    }
}

// These cover only the pure predicate on well-formed values. The actual
// hazard (a struct whose layout came from another toolchain) cannot be
// constructed in safe Rust, so the `len <= capacity` conjunct is untestable
// here. Adding a field to `PluginMetadata` deliberately breaks `valid()`, so
// that the new field has to be considered in `is_metadata_plausible`.
#[cfg(test)]
mod tests {
    use super::{MAX_METADATA_LIST_LEN, MAX_METADATA_TEXT_LEN, is_metadata_plausible};
    use crate::plugin::PluginMetadata;

    fn valid() -> PluginMetadata {
        PluginMetadata {
            name: "example".to_string(),
            version: "1.0.0".to_string(),
            authors: vec!["someone".to_string()],
            description: "an example plugin".to_string(),
            dependencies: Vec::new(),
            permissions: Vec::new(),
        }
    }

    #[test]
    fn accepts_normal_metadata() {
        assert!(is_metadata_plausible(&valid()));
    }

    #[test]
    fn rejects_empty_name() {
        let mut metadata = valid();
        metadata.name = String::new();
        assert!(!is_metadata_plausible(&metadata));
    }

    #[test]
    fn rejects_oversized_text_field() {
        let mut metadata = valid();
        metadata.description = "x".repeat(MAX_METADATA_TEXT_LEN + 1);
        assert!(!is_metadata_plausible(&metadata));
    }

    #[test]
    fn rejects_oversized_list() {
        let mut metadata = valid();
        metadata.authors = vec![String::new(); MAX_METADATA_LIST_LEN + 1];
        assert!(!is_metadata_plausible(&metadata));
    }

    #[test]
    fn rejects_oversized_string_inside_a_list() {
        let mut metadata = valid();
        metadata.authors = vec!["y".repeat(MAX_METADATA_TEXT_LEN + 1)];
        assert!(!is_metadata_plausible(&metadata));
    }

    #[test]
    fn accepts_field_at_the_size_limit() {
        let mut metadata = valid();
        metadata.description = "x".repeat(MAX_METADATA_TEXT_LEN);
        metadata.permissions = vec!["z".repeat(MAX_METADATA_TEXT_LEN); 2];
        assert!(is_metadata_plausible(&metadata));
    }
}
