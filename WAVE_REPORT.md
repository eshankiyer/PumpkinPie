# Parity wave report

| Diff | Result | Current coverage or port | Vanilla source re-verified |
| --- | --- | --- | --- |
| `coral-fixes.diff` | PORTED | Corrected coral mushroom origin-relative positions and short-circuited RNG draws in `crates/pumpkin-world/src/generation/feature/features/coral/coral_mushroom.rs`; added waterlogged self-checks and neighbor-update death scheduling for coral fans/plants in `crates/pumpkin/src/block/blocks/coral/`. | `CoralMushroomFeature.java:22-34`; `BaseCoralPlantTypeBlock.java:35-54`; `CoralFanBlock.java:33-65`; `CoralPlantBlock.java:37-69` |
| `locate.diff` | NOT_NEEDED | Already covered by `crates/pumpkin/src/command/commands/locate.rs`, including structure registration, permission, resolution, search, feedback, and distance handling. | `LocateCommand.java:50-108`; `LocateCommand.java:165-187` |
| `villager-check.diff` | NOT_NEEDED | Already covered by `crates/pumpkin/src/entity/ai/goal/villager_schedule.rs`, `crates/pumpkin/src/entity/passive/villager/mod.rs`, `crates/pumpkin/src/entity/mob/mod.rs`, and `crates/pumpkin/src/world/mod.rs`, including meeting-point memory, POI validation/release, and death cleanup. | `VillagerGoalPackages.java:43-60`; `VillagerGoalPackages.java:147-171`; `ValidateNearbyPoi.java:19-41`; `Villager.java:596-613` |

