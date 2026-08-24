pub mod function_loader;
pub mod recipe_loader;

use std::cell::Cell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use pumpkin_data::game_rules::{GameRule, GameRuleRegistry, GameRuleValue};
use pumpkin_protocol::codec::recipe::DynamicRecipe;

use crate::command::context::command_source::CommandSource;
use crate::server::Server;
use crate::server::recipe::RecipeManager;

tokio::task_local! {
    /// Remaining command quota of the currently running function chain.
    ///
    /// Vanilla executes every function line through an `ExecutionContext` holding a
    /// `Deque<CommandQueueEntry>` and a `commandQuota` decremented once per queued
    /// command; when the quota reaches zero the whole chain stops
    /// (`ExecutionContext.java:88-92`, entries run at `:100-102`). The limit comes
    /// from the `max_command_sequence_length` gamerule, read when the outermost
    /// execution context is created (`Commands.java:399-403`). Vanilla keeps that
    /// context in a thread-local so nested calls join the ambient chain instead of
    /// getting a fresh quota (`Commands.java:396-414`). A tokio task-local is the
    /// direct analogue here: every nesting level between `execute_function` and the
    /// dispatcher runs on one task through inline `.await`s.
    static REMAINING_COMMAND_QUOTA: Cell<i64>;
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

pub struct DatapackManager {
    loaded_packs: RwLock<Vec<LoadedDatapack>>,
    functions: RwLock<HashMap<String, Vec<String>>>,
    function_tags: RwLock<HashMap<String, Vec<String>>>,
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
        }
    }

    pub async fn load_all(
        &self,
        world_path: &Path,
        enabled_packs: &[String],
        recipe_manager: &RecipeManager,
    ) {
        let datapacks_dir = world_path.join("datapacks");
        let mut loaded_packs_vec = Vec::new();
        let mut all_recipes: Vec<DynamicRecipe> = Vec::new();
        let mut all_functions: HashMap<String, Vec<String>> = HashMap::new();
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

        recipe_manager.set_recipes(all_recipes).await;
        *self.loaded_packs.write().await = loaded_packs_vec;
        *self.functions.write().await = all_functions;
        *self.function_tags.write().await = all_function_tags;
    }

    pub async fn get_loaded_packs(&self) -> Vec<LoadedDatapack> {
        self.loaded_packs.read().await.clone()
    }

    pub async fn get_functions(&self) -> HashMap<String, Vec<String>> {
        self.functions.read().await.clone()
    }

    pub async fn get_function_names(&self) -> Vec<String> {
        let fns = self.functions.read().await;
        let tags = self.function_tags.read().await;
        let mut names = Vec::with_capacity(fns.len() + tags.len());
        names.extend(fns.keys().cloned());
        for tag in tags.keys() {
            names.push(format!("#{tag}"));
        }
        names
    }

    pub async fn execute_function(
        &self,
        server: &Arc<Server>,
        source: &CommandSource,
        name: &str,
    ) -> Result<usize, String> {
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
        if REMAINING_COMMAND_QUOTA.try_with(Cell::get).is_ok() {
            return self
                .execute_function_in_context(server, source, name, limit)
                .await;
        }

        REMAINING_COMMAND_QUOTA
            .scope(
                Cell::new(limit),
                self.execute_function_in_context(server, source, name, limit),
            )
            .await
    }

    /// Runs one function or function tag inside the current command chain.
    ///
    /// Port of the per-entry execution of vanilla's command queue
    /// (`ExecutionContext.runCommandQueue`, `ExecutionContext.java:88-105`,
    /// entries dispatched through `CommandQueueEntry.execute`,
    /// `CommandQueueEntry.java:4-6`): every executed line costs one unit of the
    /// chain quota, and an exhausted quota aborts the whole remaining chain with
    /// vanilla's log message.
    async fn execute_function_in_context(
        &self,
        server: &Arc<Server>,
        source: &CommandSource,
        name: &str,
        limit: i64,
    ) -> Result<usize, String> {
        let (functions_to_run, is_tag) = if let Some(tag_name) = name.strip_prefix('#') {
            let tags = self.function_tags.read().await;
            let Some(fns) = tags.get(tag_name) else {
                return Err(format!("Unknown function tag: #{tag_name}"));
            };
            (fns.clone(), true)
        } else {
            (vec![name.to_string()], false)
        };

        let all_fns = self.functions.read().await;
        let mut total_executed = 0;

        for fn_id in functions_to_run {
            let Some(lines) = all_fns.get(&fn_id) else {
                if !is_tag {
                    return Err(format!("Unknown function: {fn_id}"));
                }
                continue;
            };

            for line in lines {
                // Vanilla checks the quota before polling each queued entry and
                // breaks out of the entire queue once it is spent
                // (`ExecutionContext.java:89-92`).
                let remaining = REMAINING_COMMAND_QUOTA.with(Cell::get);
                if remaining <= 0 {
                    // Vanilla logs the configured limit here
                    // (`ExecutionContext.java:89-91`).
                    info!("Command execution stopped due to limit (executed {limit} commands)");
                    return Ok(total_executed);
                }
                REMAINING_COMMAND_QUOTA.with(|quota| quota.set(remaining - 1));

                server
                    .command_dispatcher
                    .load()
                    .handle_command(source, line)
                    .await;
                total_executed += 1;
            }
        }

        Ok(total_executed)
    }
}

fn read_pack_mcmeta(pack_path: &Path) -> (String, u32) {
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
