pub mod function_loader;
pub mod macro_function;
pub mod recipe_loader;

use std::cell::Cell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use tracing::{info, warn};

use pumpkin_data::game_rules::{GameRule, GameRuleRegistry, GameRuleValue};
use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::recipe::DynamicRecipe;
use pumpkin_util::text::TextComponent;

use crate::command::context::command_source::CommandSource;
use crate::server::Server;
use crate::server::recipe::RecipeManager;

use macro_function::{InstantiationCache, InstantiationError, LoadedFunction};

std::thread_local! {
    /// Remaining command quota of the currently running function chain.
    ///
    /// Vanilla executes every function line through an `ExecutionContext` holding a
    /// `Deque<CommandQueueEntry>` and a `commandQuota` decremented once per queued
    /// command; when the quota reaches zero the whole chain stops
    /// (`ExecutionContext.java:88-92`, entries run at `:100-102`). The limit comes
    /// from the `max_command_sequence_length` gamerule, read when the outermost
    /// execution context is created (`Commands.java:399-403`). Vanilla keeps that
    /// context in a thread-local so nested calls join the ambient chain instead of
    /// getting a fresh quota (`Commands.java:396-414`). The synchronous dispatcher
    /// uses the same thread-local boundary for nested executions.
    static REMAINING_COMMAND_QUOTA: Cell<Option<i64>> = const { Cell::new(None) };
}

#[derive(Clone, Debug)]
pub struct LoadedDatapack {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pack_format: u32,
    pub root_path: PathBuf,
    pub recipe_count: usize,
    pub function_count: usize,
}

/// Why [`DatapackManager::execute_function`] failed.
#[derive(Debug)]
pub enum ExecuteFunctionError {
    /// No function or function tag exists under this id (vanilla
    /// `arguments.function.unknown`, raised by `FunctionArgument`).
    Unknown(String),
    /// Macro instantiation failed. Port of
    /// `ERROR_FUNCTION_INSTANTATION_FAILURE`
    /// (`FunctionCommand.java:49-51`, raised at `:139-141`) wrapping the
    /// translated `MacroFunction.instantiate` failure
    /// (`MacroFunction.java:52-82`).
    InstantiationFailure {
        function_id: String,
        reason: TextComponent,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatapackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pack_format: u32,
    pub is_enabled: bool,
    pub recipe_count: usize,
    pub function_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DatapackEnablePosition {
    First,
    Last,
    Before(String),
    After(String),
}

pub struct DatapackManager {
    loaded_packs: RwLock<Vec<LoadedDatapack>>,
    functions: RwLock<HashMap<String, LoadedFunction>>,
    function_tags: RwLock<HashMap<String, Vec<String>>>,
    /// Per-function LRU caches of instantiated command lines, cleared whenever
    /// datapacks reload — the analogue of the cache embedded in each vanilla
    /// `MacroFunction` object (`MacroFunction.java:36`), whose lifetime also
    /// ends on reload.
    instantiation_caches: Mutex<HashMap<String, InstantiationCache>>,
}

impl Default for DatapackManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DatapackManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            loaded_packs: RwLock::new(Vec::new()),
            functions: RwLock::new(HashMap::new()),
            function_tags: RwLock::new(HashMap::new()),
            instantiation_caches: Mutex::new(HashMap::new()),
        }
    }

    pub fn load_all(
        &self,
        world_path: &Path,
        enabled_packs: &[String],
        recipe_manager: &RecipeManager,
    ) {
        let datapacks_dir = world_path.join("datapacks");
        let mut loaded_packs_vec = Vec::new();
        let mut all_recipes: Vec<DynamicRecipe> = Vec::new();
        let mut all_functions: HashMap<String, LoadedFunction> = HashMap::new();
        let mut all_function_tags: HashMap<String, Vec<String>> = HashMap::new();

        if datapacks_dir.is_dir() {
            let Ok(entries) = fs::read_dir(&datapacks_dir) else {
                warn!(
                    "Failed to read datapacks directory: {}",
                    datapacks_dir.display()
                );
                return;
            };

            for entry in entries.flatten() {
                let pack_path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();

                if file_name.starts_with('.') || !pack_path.is_dir() {
                    continue;
                }

                let pack_id = format!("file/{file_name}");
                let is_enabled = enabled_packs
                    .iter()
                    .any(|p| p == &pack_id || p == &file_name);
                if !is_enabled {
                    continue;
                }

                let (description, pack_format) = read_pack_mcmeta(&pack_path);

                let data_dir = pack_path.join("data");
                let mut pack_recipe_count = 0;
                let mut pack_function_count = 0;

                if data_dir.is_dir()
                    && let Ok(ns_entries) = fs::read_dir(&data_dir)
                {
                    for ns_entry in ns_entries.flatten() {
                        let ns_path = ns_entry.path();
                        if !ns_path.is_dir() {
                            continue;
                        }
                        let namespace = ns_entry.file_name().to_string_lossy().to_string();

                        // Load recipes
                        for recipe_sub in ["recipe", "recipes"] {
                            let recipe_dir = ns_path.join(recipe_sub);
                            if recipe_dir.is_dir() {
                                load_recipes_from_dir(
                                    &namespace,
                                    &recipe_dir,
                                    &mut all_recipes,
                                    &mut pack_recipe_count,
                                );
                            }
                        }

                        // Load functions
                        for fn_sub in ["function", "functions"] {
                            let fn_dir = ns_path.join(fn_sub);
                            if fn_dir.is_dir() {
                                let before = all_functions.len();
                                function_loader::load_functions_from_dir(
                                    &namespace,
                                    &fn_dir,
                                    &mut all_functions,
                                );
                                pack_function_count += all_functions.len() - before;
                            }
                        }

                        // Load tags
                        let tags_dir = ns_path.join("tags");
                        if tags_dir.is_dir() {
                            function_loader::load_function_tags_from_dir(
                                &namespace,
                                &tags_dir,
                                &mut all_function_tags,
                            );
                        }
                    }
                }

                info!(
                    "Loaded datapack '{file_name}': {pack_recipe_count} recipe(s), {pack_function_count} function(s)"
                );

                loaded_packs_vec.push(LoadedDatapack {
                    id: pack_id,
                    name: file_name,
                    description,
                    pack_format,
                    root_path: pack_path,
                    recipe_count: pack_recipe_count,
                    function_count: pack_function_count,
                });
            }
        }

        recipe_manager.set_recipes(all_recipes);
        *self
            .loaded_packs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = loaded_packs_vec;
        *self
            .functions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_functions;
        *self
            .function_tags
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all_function_tags;
        // Instantiations of the old function set are invalid now; vanilla gets
        // this for free because each reload builds fresh MacroFunction objects.
        self.instantiation_caches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn get_loaded_packs(&self) -> Vec<LoadedDatapack> {
        self.loaded_packs
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn get_functions(&self) -> HashMap<String, LoadedFunction> {
        self.functions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn get_function_names(&self) -> Vec<String> {
        let fns = self
            .functions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tags = self
            .function_tags
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut names = Vec::with_capacity(fns.len() + tags.len());
        names.extend(fns.keys().cloned());
        for tag in tags.keys() {
            names.push(format!("#{tag}"));
        }
        names
    }

    /// Executes a function or function tag synchronously.
    pub fn execute_function(
        &self,
        server: &Arc<Server>,
        source: &CommandSource,
        name: &str,
    ) -> Result<usize, String> {
        self.execute_function_with_arguments(server, source, name, None)
            .map_err(|error| format!("{error:?}"))
    }

    /// Executes a function or function tag with optional macro arguments.
    ///
    /// The public three-argument method follows upstream’s synchronous API; this
    /// companion retains the fork’s vanilla `/function <name> <compound>` behavior.
    pub fn execute_function_with_arguments(
        &self,
        server: &Arc<Server>,
        source: &CommandSource,
        name: &str,
        arguments: Option<&NbtCompound>,
    ) -> Result<usize, ExecuteFunctionError> {
        // Vanilla clamps the chain limit to at least 1
        // (`Commands.java:400`).
        let fallback_registry = GameRuleRegistry::default();
        let default_limit = fallback_registry.max_command_sequence_length.max(1);
        let limit = source.world.as_ref().map_or(default_limit, |world| {
            match world.get_game_rule(&GameRule::MaxCommandSequenceLength) {
                GameRuleValue::Int(v) => v.max(1),
                GameRuleValue::Bool(_) => default_limit,
            }
        });

        // A nested call (function invoking `function ...`) joins the ambient
        // chain and shares its quota, like vanilla reuses the ambient
        // ExecutionContext (`Commands.java:412-414`).
        if REMAINING_COMMAND_QUOTA.with(|quota| quota.get().is_some()) {
            return self.execute_function_in_context(server, source, name, arguments, limit);
        }

        REMAINING_COMMAND_QUOTA.with(|quota| {
            quota.set(Some(limit));
            let result = self.execute_function_in_context(server, source, name, arguments, limit);
            quota.set(None);
            result
        })
    }

    /// Runs one function or function tag inside the current command chain.
    ///
    /// Port of the per-entry execution of vanilla's command queue
    /// (`ExecutionContext.runCommandQueue`, `ExecutionContext.java:88-105`,
    /// entries dispatched through `CommandQueueEntry.execute`,
    /// `CommandQueueEntry.java:4-6`): every executed line costs one unit of the
    /// chain quota, and an exhausted quota aborts the whole remaining chain with
    /// vanilla's log message. Functions containing `$` macro lines are
    /// instantiated first (`MacroFunction.instantiate`,
    /// `MacroFunction.java:52-82`).
    fn execute_function_in_context(
        &self,
        server: &Arc<Server>,
        source: &CommandSource,
        name: &str,
        arguments: Option<&NbtCompound>,
        limit: i64,
    ) -> Result<usize, ExecuteFunctionError> {
        let (functions_to_run, is_tag) = if let Some(tag_name) = name.strip_prefix('#') {
            let tags = self
                .function_tags
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(fns) = tags.get(tag_name) else {
                return Err(ExecuteFunctionError::Unknown(format!("#{tag_name}")));
            };
            (fns.clone(), true)
        } else {
            (vec![name.to_string()], false)
        };

        let all_fns = self
            .functions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut total_executed = 0;

        for fn_id in functions_to_run {
            let Some(function) = all_fns.get(&fn_id) else {
                if !is_tag {
                    return Err(ExecuteFunctionError::Unknown(fn_id));
                }
                continue;
            };

            let lines = self.instantiate_lines(&fn_id, function, arguments)?;
            for line in lines {
                // Vanilla checks the quota before polling each queued entry and
                // breaks out of the entire queue once it is spent
                // (`ExecutionContext.java:89-92`).
                let remaining = REMAINING_COMMAND_QUOTA.with(|quota| quota.get().unwrap_or(0));
                if remaining <= 0 {
                    // Vanilla logs the configured limit here
                    // (`ExecutionContext.java:89-91`).
                    info!("Command execution stopped due to limit (executed {limit} commands)");
                    return Ok(total_executed);
                }
                REMAINING_COMMAND_QUOTA.with(|quota| quota.set(Some(remaining - 1)));

                server
                    .command_dispatcher
                    .load()
                    .handle_command(source, line);
                total_executed += 1;
            }
        }

        Ok(total_executed)
    }

    /// Instantiates a function into the concrete command lines to dispatch.
    ///
    /// Plain functions pass their lines through unchanged (vanilla
    /// `PlainTextFunction.instantiate`). Macro functions require an argument
    /// compound and run through the LRU cache exactly like vanilla's
    /// `MacroFunction.instantiate` (`MacroFunction.java:52-82`): missing
    /// arguments raise an instantiation error which the caller surfaces as
    /// `commands.function.instantiationFailure`
    /// (`FunctionCommand.java:49-51`, `:139-141`), parameter values are pulled
    /// and stringified in parameter order (`:57-68`, stringify at `:84-94`),
    /// cached hits skip re-substitution (`:70-73`) and misses substitute all
    /// entries before being inserted, evicting the least-recently-used entry
    /// once 8 slots are occupied (`:75-81`).
    fn instantiate_lines(
        &self,
        fn_id: &str,
        function: &LoadedFunction,
        arguments: Option<&NbtCompound>,
    ) -> Result<Vec<String>, ExecuteFunctionError> {
        if !function.is_macro() {
            return Ok(function
                .lines
                .iter()
                .filter_map(|line| match line {
                    macro_function::FunctionLine::Plain(command) => Some(command.clone()),
                    // A function without parameters cannot contain macro lines;
                    // guaranteed by `LoadedFunction::from_lines`.
                    macro_function::FunctionLine::Macro(_) => None,
                })
                .collect());
        }

        let instantiation_error = |error| instantiation_failure(fn_id, error);

        let arguments =
            arguments.ok_or_else(|| instantiation_error(InstantiationError::MissingArguments))?;
        let values = function
            .extract_parameter_values(arguments)
            .map_err(instantiation_error)?;

        let mut caches = self
            .instantiation_caches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cache = caches.entry(fn_id.to_string()).or_default();
        cache
            .get_or_insert_with(&values, || function.substitute_all(&values))
            .map_err(|e| ExecuteFunctionError::InstantiationFailure {
                function_id: fn_id.to_string(),
                reason: TextComponent::text(e),
            })
    }
}

/// Translates a macro instantiation failure into the user-facing component
/// vanilla produces before wrapping it in `ERROR_FUNCTION_INSTANTATION_FAILURE`
/// (`FunctionCommand.java:139-141`): "missing arguments to function %s"
/// (`MacroFunction.java:53-55`) or "missing argument for function %s, %s"
/// (`MacroFunction.java:61-65`).
fn instantiation_failure(fn_id: &str, error: InstantiationError) -> ExecuteFunctionError {
    let reason = match error {
        InstantiationError::MissingArguments => TextComponent::translate_cross(
            translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS,
            translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS,
            [TextComponent::text(fn_id.to_string())],
        ),
        InstantiationError::MissingArgument(parameter) => TextComponent::translate_cross(
            translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT,
            translation::java::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT,
            [
                TextComponent::text(fn_id.to_string()),
                TextComponent::text(parameter),
            ],
        ),
    };
    ExecuteFunctionError::InstantiationFailure {
        function_id: fn_id.to_string(),
        reason,
    }
}

impl DatapackManager {
    pub fn get_all_known_packs(server: &Server) -> Vec<String> {
        let mut packs = Vec::new();
        packs.push("vanilla".to_string());

        // Bundled feature packs
        for bundled in [
            "trade_rebalance",
            "minecart_improvements",
            "redstone_experiments",
        ] {
            if !packs.iter().any(|p| p == bundled) {
                packs.push(bundled.to_string());
            }
        }

        // World datapacks directory
        let datapacks_dir = server.basic_config.get_world_path().join("datapacks");
        if let Ok(entries) = fs::read_dir(datapacks_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') {
                    continue;
                }
                if path.is_dir()
                    || path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                {
                    let pack_name = format!("file/{file_name}");
                    if !packs.iter().any(|p| p == &pack_name) {
                        packs.push(pack_name);
                    }
                }
            }
        }

        let level_info = server.level_info.load();
        for pack in &level_info.data_packs.enabled {
            if !packs.iter().any(|p| p == pack) {
                packs.push(pack.clone());
            }
        }
        for pack in &level_info.data_packs.disabled {
            if !packs.iter().any(|p| p == pack) {
                packs.push(pack.clone());
            }
        }

        packs
    }

    pub fn get_enabled_packs(server: &Server) -> Vec<String> {
        server.level_info.load().data_packs.enabled.clone()
    }

    pub fn get_available_packs(server: &Server) -> Vec<String> {
        let enabled = Self::get_enabled_packs(server);
        let all = Self::get_all_known_packs(server);
        all.into_iter().filter(|p| !enabled.contains(p)).collect()
    }

    pub fn find_pack_name(server: &Server, input: &str) -> Option<String> {
        let known = Self::get_all_known_packs(server);
        if let Some(p) = known.iter().find(|p| *p == input) {
            return Some(p.clone());
        }
        let file_input = format!("file/{input}");
        if let Some(p) = known.iter().find(|p| **p == file_input) {
            return Some(p.clone());
        }
        if let Some(p) = known
            .iter()
            .find(|p| p.strip_prefix("file/") == Some(input))
        {
            return Some(p.clone());
        }
        None
    }

    pub fn get_pack_info(server: &Server, name_or_id: &str) -> Option<DatapackInfo> {
        let resolved_name = Self::find_pack_name(server, name_or_id)?;
        let enabled_packs = Self::get_enabled_packs(server);
        let is_enabled = enabled_packs.contains(&resolved_name);

        let loaded = server.datapack_manager.get_loaded_packs();
        if let Some(pack) = loaded
            .iter()
            .find(|p| p.id == resolved_name || p.name == resolved_name)
        {
            return Some(DatapackInfo {
                id: pack.id.clone(),
                name: pack.name.clone(),
                description: pack.description.clone(),
                pack_format: pack.pack_format,
                is_enabled,
                recipe_count: pack.recipe_count,
                function_count: pack.function_count,
            });
        }

        let (id, name, description, pack_format) = if resolved_name == "vanilla" {
            (
                "vanilla".to_string(),
                "vanilla".to_string(),
                "The default data pack".to_string(),
                61,
            )
        } else if let Some(stripped) = resolved_name.strip_prefix("file/") {
            let pack_path = server
                .basic_config
                .get_world_path()
                .join("datapacks")
                .join(stripped);
            let (desc, format) = read_pack_mcmeta(&pack_path);
            (resolved_name.clone(), stripped.to_string(), desc, format)
        } else {
            (
                resolved_name.clone(),
                resolved_name.clone(),
                format!("Bundled datapack: {resolved_name}"),
                61,
            )
        };

        Some(DatapackInfo {
            id,
            name,
            description,
            pack_format,
            is_enabled,
            recipe_count: 0,
            function_count: 0,
        })
    }

    pub fn list_all_packs(server: &Server) -> Vec<DatapackInfo> {
        let all = Self::get_all_known_packs(server);
        all.into_iter()
            .filter_map(|p| Self::get_pack_info(server, &p))
            .collect()
    }

    pub fn list_enabled_packs(server: &Server) -> Vec<DatapackInfo> {
        let enabled = Self::get_enabled_packs(server);
        enabled
            .into_iter()
            .filter_map(|p| Self::get_pack_info(server, &p))
            .collect()
    }

    pub fn list_available_packs(server: &Server) -> Vec<DatapackInfo> {
        let available = Self::get_available_packs(server);
        available
            .into_iter()
            .filter_map(|p| Self::get_pack_info(server, &p))
            .collect()
    }

    pub fn is_pack_enabled(server: &Server, name: &str) -> bool {
        let Some(resolved) = Self::find_pack_name(server, name) else {
            return false;
        };
        Self::get_enabled_packs(server).contains(&resolved)
    }

    pub fn enable_pack(
        server: &Arc<Server>,
        name: &str,
        position: DatapackEnablePosition,
    ) -> Result<(), String> {
        let Some(resolved_name) = Self::find_pack_name(server, name) else {
            return Err(format!("Unknown datapack '{name}'"));
        };

        let enabled = Self::get_enabled_packs(server);
        if enabled.contains(&resolved_name) {
            return Err(format!("Datapack '{resolved_name}' is already enabled"));
        }

        let target = resolved_name;
        match position {
            DatapackEnablePosition::First => {
                server.level_info.rcu(|level_info| {
                    let mut new_info = (**level_info).clone();
                    new_info.data_packs.disabled.retain(|p| p != &target);
                    new_info.data_packs.enabled.retain(|p| p != &target);
                    new_info.data_packs.enabled.insert(0, target.clone());
                    new_info
                });
            }
            DatapackEnablePosition::Last => {
                server.level_info.rcu(|level_info| {
                    let mut new_info = (**level_info).clone();
                    new_info.data_packs.disabled.retain(|p| p != &target);
                    new_info.data_packs.enabled.retain(|p| p != &target);
                    new_info.data_packs.enabled.push(target.clone());
                    new_info
                });
            }
            DatapackEnablePosition::Before(existing_name) => {
                let Some(existing_pack) = Self::find_pack_name(server, &existing_name) else {
                    return Err(format!("Unknown existing datapack '{existing_name}'"));
                };
                if !enabled.contains(&existing_pack) {
                    return Err(format!("Datapack '{existing_pack}' is not enabled"));
                }
                server.level_info.rcu(|level_info| {
                    let mut new_info = (**level_info).clone();
                    new_info.data_packs.disabled.retain(|p| p != &target);
                    new_info.data_packs.enabled.retain(|p| p != &target);
                    if let Some(idx) = new_info
                        .data_packs
                        .enabled
                        .iter()
                        .position(|p| p == &existing_pack)
                    {
                        new_info.data_packs.enabled.insert(idx, target.clone());
                    } else {
                        new_info.data_packs.enabled.push(target.clone());
                    }
                    new_info
                });
            }
            DatapackEnablePosition::After(existing_name) => {
                let Some(existing_pack) = Self::find_pack_name(server, &existing_name) else {
                    return Err(format!("Unknown existing datapack '{existing_name}'"));
                };
                if !enabled.contains(&existing_pack) {
                    return Err(format!("Datapack '{existing_pack}' is not enabled"));
                }
                server.level_info.rcu(|level_info| {
                    let mut new_info = (**level_info).clone();
                    new_info.data_packs.disabled.retain(|p| p != &target);
                    new_info.data_packs.enabled.retain(|p| p != &target);
                    if let Some(idx) = new_info
                        .data_packs
                        .enabled
                        .iter()
                        .position(|p| p == &existing_pack)
                    {
                        new_info.data_packs.enabled.insert(idx + 1, target.clone());
                    } else {
                        new_info.data_packs.enabled.push(target.clone());
                    }
                    new_info
                });
            }
        }

        if let Err(err) = server.save_world_info() {
            tracing::error!("Failed to save world info: {err}");
        }

        server.reload_datapacks(server);
        Ok(())
    }

    pub fn disable_pack(server: &Arc<Server>, name: &str) -> Result<(), String> {
        let Some(target_pack) = Self::find_pack_name(server, name) else {
            return Err(format!("Unknown datapack '{name}'"));
        };

        let enabled = Self::get_enabled_packs(server);
        if !enabled.contains(&target_pack) {
            return Err(format!("Datapack '{target_pack}' is not enabled"));
        }

        if target_pack == "vanilla" {
            return Err("Cannot disable the default vanilla datapack".to_string());
        }

        let target = target_pack;
        server.level_info.rcu(|level_info| {
            let mut new_info = (**level_info).clone();
            new_info.data_packs.enabled.retain(|p| p != &target);
            if !new_info.data_packs.disabled.contains(&target) {
                new_info.data_packs.disabled.push(target.clone());
            }
            new_info
        });

        if let Err(err) = server.save_world_info() {
            tracing::error!("Failed to save world info: {err}");
        }

        server.reload_datapacks(server);
        Ok(())
    }

    pub fn reload(server: &Arc<Server>) -> Result<(), String> {
        server.reload_datapacks(server);
        Ok(())
    }

    pub fn execute_function_from_console(
        server: &Arc<Server>,
        name: &str,
    ) -> Result<usize, String> {
        let source = crate::command::CommandSender::Console.into_source(server);
        server
            .datapack_manager
            .execute_function(server, &source, name)
    }
}

pub fn read_pack_mcmeta(pack_path: &Path) -> (String, u32) {
    let mcmeta_path = pack_path.join("pack.mcmeta");
    if let Ok(content) = fs::read_to_string(mcmeta_path)
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let pack = val.get("pack");
        let description = pack
            .and_then(|p| p.get("description"))
            .map(|d| {
                d.as_str()
                    .map_or_else(|| d.to_string(), ToString::to_string)
            })
            .unwrap_or_default();
        let pack_format = pack
            .and_then(|p| p.get("pack_format"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(61) as u32;
        return (description, pack_format);
    }
    (String::new(), 61)
}

fn load_recipes_from_dir(
    namespace: &str,
    dir: &Path,
    all_recipes: &mut Vec<DynamicRecipe>,
    count: &mut usize,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_recipes_from_dir(namespace, &path, all_recipes, count);
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Ok(content) = fs::read_to_string(&path)
                && let Some(recipe) = recipe_loader::parse_recipe(namespace, &stem, &content)
            {
                all_recipes.push(recipe);
                *count += 1;
            }
        }
    }
}
