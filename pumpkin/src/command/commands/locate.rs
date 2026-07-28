//! The `/locate` command.
//!
//! Scaffolded deliberately: the pure, testable pieces are stubs with `todo!()`
//! bodies and a unit test each that defines correct behaviour, so the local
//! model can fill them one at a time under the normal build-and-test gate. The
//! executor that needs a live `World` is wired by hand once the pure pieces
//! pass, because an oracle probe cannot see any of this until the command is
//! registered end to end.
//!
//! Landing order, which the specs encode as a chain:
//!   1. `structure_by_name`      no dependencies
//!   2. `sets_containing`        no dependencies
//!   3. `horizontal_distance`    no dependencies
//!   4. executor                 needs all three

use std::pin::Pin;

use pumpkin_data::structures::{StructureKeys, StructureSet};
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::string_reader::StringReader;
use crate::command::suggestion::suggestions::{Suggestions, SuggestionsBuilder};

const DESCRIPTION: &str = "Locate the nearest generated structure.";
const PERMISSION: &str = "minecraft:command.locate";

const ARG_STRUCTURE: &str = "structure";

/// Vanilla searches 100 chunk-region rings out from the origin before giving up.
#[allow(
    dead_code,
    reason = "used once the executor is wired, after specs 20 to 22"
)]
pub const MAX_SEARCH_RADIUS: i32 = 100;

static ERROR_INVALID_STRUCTURE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_LOCATE_STRUCTURE_INVALID,
    translation::java::COMMANDS_LOCATE_STRUCTURE_INVALID,
);
static ERROR_NOT_FOUND: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_LOCATE_STRUCTURE_NOT_FOUND,
    translation::java::COMMANDS_LOCATE_STRUCTURE_NOT_FOUND,
);

/// Every structure key, by its registry path.
///
/// `StructureSet::get` is keyed by structure SET name (`villages`, `strongholds`),
/// but `/locate structure` takes a structure name (`village_plains`), and the
/// generated `StructureKeys` enum has no name lookup. Rather than hand-edit
/// generated code, the mapping lives here next to its only consumer.
pub const STRUCTURE_NAMES: &[(&str, StructureKeys)] = &[
    ("pillager_outpost", StructureKeys::PillagerOutpost),
    ("mineshaft", StructureKeys::Mineshaft),
    ("mineshaft_mesa", StructureKeys::MineshaftMesa),
    ("mansion", StructureKeys::Mansion),
    ("jungle_pyramid", StructureKeys::JunglePyramid),
    ("desert_pyramid", StructureKeys::DesertPyramid),
    ("igloo", StructureKeys::Igloo),
    ("shipwreck", StructureKeys::Shipwreck),
    ("shipwreck_beached", StructureKeys::ShipwreckBeached),
    ("swamp_hut", StructureKeys::SwampHut),
    ("stronghold", StructureKeys::Stronghold),
    ("monument", StructureKeys::Monument),
    ("ocean_ruin_cold", StructureKeys::OceanRuinCold),
    ("ocean_ruin_warm", StructureKeys::OceanRuinWarm),
    ("fortress", StructureKeys::Fortress),
    ("nether_fossil", StructureKeys::NetherFossil),
    ("end_city", StructureKeys::EndCity),
    ("buried_treasure", StructureKeys::BuriedTreasure),
    ("bastion_remnant", StructureKeys::BastionRemnant),
    ("village_plains", StructureKeys::VillagePlains),
    ("village_desert", StructureKeys::VillageDesert),
    ("village_savanna", StructureKeys::VillageSavanna),
    ("village_snowy", StructureKeys::VillageSnowy),
    ("village_taiga", StructureKeys::VillageTaiga),
    ("ruined_portal", StructureKeys::RuinedPortal),
    ("ruined_portal_desert", StructureKeys::RuinedPortalDesert),
    ("ruined_portal_jungle", StructureKeys::RuinedPortalJungle),
    ("ruined_portal_swamp", StructureKeys::RuinedPortalSwamp),
    (
        "ruined_portal_mountain",
        StructureKeys::RuinedPortalMountain,
    ),
    ("ruined_portal_ocean", StructureKeys::RuinedPortalOcean),
    ("ruined_portal_nether", StructureKeys::RuinedPortalNether),
    ("ancient_city", StructureKeys::AncientCity),
    ("trail_ruins", StructureKeys::TrailRuins),
    ("trial_chambers", StructureKeys::TrialChambers),
];

/// Resolve a structure argument to its key.
///
/// Accepts a bare path (`village_plains`) or a namespaced identifier
/// (`minecraft:village_plains`). Any other namespace is not a vanilla structure
/// and must not resolve.
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 20; remove this attribute when filling"
)]
pub fn structure_by_name(name: &str) -> Option<StructureKeys> {
    todo!("STUB: see tests::structure_by_name_*")
}

/// The structure sets that can generate this structure.
///
/// A set is one placement rule plus a weighted list of structures, so `villages`
/// governs all five village variants. Locating one variant means searching every
/// set that lists it.
#[allow(dead_code, reason = "used once the executor is wired")]
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 21; remove this attribute when filling"
)]
pub fn sets_containing(key: StructureKeys) -> Vec<&'static StructureSet> {
    todo!("STUB: see tests::sets_containing_*")
}

/// Horizontal distance in blocks, rounded, which is what vanilla reports.
///
/// Y is ignored because the search itself is horizontal and the finder returns a
/// column whose height is not known without generating the chunk.
#[allow(dead_code, reason = "used once the executor is wired")]
#[expect(
    clippy::todo,
    unused_variables,
    reason = "scaffold stub, filled by spec 22; remove this attribute when filling"
)]
pub fn horizontal_distance(a: BlockPos, b: BlockPos) -> i32 {
    todo!("STUB: see tests::horizontal_distance_*")
}

/// A structure name argument.
///
/// Vanilla's parser also accepts a tag here (`#minecraft:village`), but
/// `pumpkin-data` has no `worldgen/structure` tag registry, so a `#` form has
/// nothing to resolve against. It is reported as invalid rather than silently
/// matching nothing.
struct StructureArgumentType;

impl ArgumentType for StructureArgumentType {
    type Item = String;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        Ok(reader.read_unquoted_string())
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext,
        suggestions_builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send>> {
        Box::pin(async move {
            suggestions_builder
                .filter_and_suggest_iter(
                    STRUCTURE_NAMES
                        .iter()
                        .map(|(name, _)| format!("minecraft:{name}")),
                )
                .build()
        })
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::ResourceOrTagKey {
            identifier: pumpkin_util::identifier::Identifier::vanilla_static("worldgen/structure"),
        }
    }
}

struct LocateStructureExecutor;

impl CommandExecutor for LocateStructureExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let name = context.get_argument::<String>(ARG_STRUCTURE)?;
            let _key = structure_by_name(name).ok_or_else(|| {
                ERROR_INVALID_STRUCTURE.create_without_context(TextComponent::text(name.clone()))
            })?;
            // Wired by hand once the three pure helpers above pass their tests.
            // Deliberately not stubbed out to the model: it needs a live World,
            // a generator that may be Flat and therefore have no structure
            // cache, and the seed, none of which a unit test can supply.
            Err(ERROR_NOT_FOUND.create_without_context(TextComponent::text(name.clone())))
        })
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &mut PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let builder = command("locate", DESCRIPTION)
        .requires(PERMISSION)
        .then(literal("structure").then(
            argument(ARG_STRUCTURE, StructureArgumentType).executes(LocateStructureExecutor),
        ));

    dispatcher.register(builder);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- structure_by_name ----------------------------------------------

    #[test]
    fn structure_by_name_accepts_bare_path() {
        assert_eq!(
            structure_by_name("village_plains"),
            Some(StructureKeys::VillagePlains)
        );
    }

    #[test]
    fn structure_by_name_accepts_vanilla_namespace() {
        assert_eq!(
            structure_by_name("minecraft:ancient_city"),
            Some(StructureKeys::AncientCity)
        );
    }

    #[test]
    fn structure_by_name_rejects_unknown() {
        assert_eq!(structure_by_name("not_a_structure"), None);
        assert_eq!(structure_by_name("minecraft:not_a_structure"), None);
    }

    #[test]
    fn structure_by_name_rejects_foreign_namespace() {
        // A modded namespace is not a vanilla structure. Stripping any prefix up
        // to the colon would wrongly accept this.
        assert_eq!(structure_by_name("othermod:village_plains"), None);
    }

    #[test]
    fn structure_by_name_covers_every_key() {
        // Every entry in the table must round-trip, which catches a typo in a
        // name that would otherwise only show up as one unlocatable structure.
        for (name, key) in STRUCTURE_NAMES {
            assert_eq!(structure_by_name(name), Some(*key), "failed for {name}");
        }
    }

    // ---- sets_containing -------------------------------------------------

    #[test]
    fn sets_containing_finds_village_set() {
        let sets = sets_containing(StructureKeys::VillagePlains);
        assert!(!sets.is_empty(), "village_plains must belong to a set");
        assert!(
            sets.iter().any(|s| s
                .structures
                .iter()
                .any(|e| e.structure == StructureKeys::VillageDesert)),
            "the set containing village_plains also lists the other variants"
        );
    }

    #[test]
    fn sets_containing_finds_stronghold_set() {
        // Strongholds use concentric-rings placement rather than random spread,
        // so this also proves the lookup is not filtering by placement type.
        let sets = sets_containing(StructureKeys::Stronghold);
        assert_eq!(sets.len(), 1);
    }

    #[test]
    fn sets_containing_every_key_resolves() {
        for (name, key) in STRUCTURE_NAMES {
            assert!(
                !sets_containing(*key).is_empty(),
                "{name} belongs to no structure set"
            );
        }
    }

    // ---- horizontal_distance ---------------------------------------------

    #[test]
    fn horizontal_distance_ignores_height() {
        let a = BlockPos::new(0, 64, 0);
        let b = BlockPos::new(3, 200, 4);
        // 3-4-5 triangle. A distance that counted Y would be much larger.
        assert_eq!(horizontal_distance(a, b), 5);
    }

    #[test]
    fn horizontal_distance_is_symmetric() {
        let a = BlockPos::new(-120, 70, 45);
        let b = BlockPos::new(310, 12, -900);
        assert_eq!(horizontal_distance(a, b), horizontal_distance(b, a));
    }

    #[test]
    fn horizontal_distance_zero_for_same_column() {
        let a = BlockPos::new(17, 5, -3);
        let b = BlockPos::new(17, 250, -3);
        assert_eq!(horizontal_distance(a, b), 0);
    }

    #[test]
    fn horizontal_distance_rounds_to_nearest() {
        // sqrt(2) * 1000 = 1414.21..., which rounds to 1414, not 1415 and not
        // 1414 by truncation of a larger value.
        let a = BlockPos::new(0, 0, 0);
        let b = BlockPos::new(1000, 0, 1000);
        assert_eq!(horizontal_distance(a, b), 1414);
    }
}
