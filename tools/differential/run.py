#!/usr/bin/env python3
"""Run identical, deterministic command probes against vanilla and Pumpkin.

This is deliberately a small first slice of the parity harness. It compares
server-observable command output and stores the exact inputs used for the run.
It does not pretend that an untested Java class is covered.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from rcon import Rcon, RconError


ROOT = Path(__file__).resolve().parents[2]


@dataclass(frozen=True)
class Target:
    name: str
    host: str
    port: int
    password: str


@dataclass(frozen=True)
class Probe:
    name: str
    commands: tuple[str, ...]
    normalizer: str
    source: tuple[str, ...]
    expected: tuple[str, ...]


PROBES = (
    Probe(
        "command_semantics",
        (
            "weather clear",
            "difficulty hard",
            "difficulty normal",
        ),
        "command_outcome",
        (
            "net/minecraft/server/commands/WeatherCommand.java",
            "net/minecraft/server/commands/DifficultyCommand.java",
            "assets/minecraft/lang/en_us.json",
        ),
        (
            "weather:clear",
            "difficulty:hard:set",
            "difficulty:normal:set",
        ),
    ),
    Probe(
        "forceload_block_mutation",
        (
            "forceload remove all",
            "forceload query 16000 16000",
            "forceload add 16000 16000",
            "forceload query 16000 16000",
            "setblock 16000 63 16000 stone",
            "execute if block 16000 63 16000 minecraft:stone run setblock 16001 63 16000 gold_block",
            "setblock 16000 63 16000 air",
            "setblock 16000 63 16000 air",
            "setblock 16001 63 16000 air",
            "forceload remove 16000 16000",
            "forceload query 16000 16000",
        ),
        "command_outcome",
        (
            "net/minecraft/server/commands/ForceLoadCommand.java",
            "net/minecraft/server/commands/SetBlockCommand.java",
            "net/minecraft/server/commands/ExecuteCommand.java",
            "assets/minecraft/lang/en_us.json",
        ),
        (
            "forceload:ok",
            "forceload:query_error",
            "forceload:ok",
            "forceload:query_ok",
            "setblock:changed",
            "execute:stone",
            "setblock:changed",
            "setblock:failed",
            "setblock:changed",
            "forceload:ok",
            "forceload:query_error",
        ),
    ),
    Probe(
        "time_and_gamerule_queries",
        (
            "time query gametime",
            "gamerule spawn_mobs true",
            "gamerule spawn_mobs",
        ),
        "command_outcome",
        (
            "net/minecraft/server/commands/TimeCommand.java",
            "net/minecraft/server/commands/GameRuleCommand.java",
            "net/minecraft/world/level/gamerules/GameRules.java",
            "assets/minecraft/lang/en_us.json",
        ),
        (
            "gametime:ok",
            "spawn_mobs:set:true",
            "spawn_mobs:true",
        ),
    ),
    Probe(
        "execute_unloaded_block_predicate",
        (
            "forceload remove all",
            "execute if block 200000 63 200000 minecraft:air run setblock 0 63 0 stone",
            "execute unless block 200000 63 200000 minecraft:stone run setblock 0 63 0 gold_block",
        ),
        "unloaded_predicate",
        (
            "net/minecraft/server/commands/ExecuteCommand.java",
            "net/minecraft/commands/arguments/coordinates/BlockPosArgument.java",
        ),
        (
            "forceload:ok",
            "",
            "",
        ),
    ),
    Probe(
        "exact_rcon_command_text",
        (
            "forceload remove all",
            "forceload add 16000 16000",
            "forceload query 16000 16000",
            "forceload remove 16000 16000",
        ),
        "exact_output",
        (
            "net/minecraft/server/rcon/RconConsoleSource.java",
            "net/minecraft/server/rcon/thread/RconClient.java",
            "net/minecraft/server/commands/ForceLoadCommand.java",
            "assets/minecraft/lang/en_us.json",
        ),
        (),
    ),
    Probe(
        "setblock_defined_properties_and_shapes",
        (
            "forceload add 16100 16100",
            "fill 16098 62 16098 16102 64 16102 air",
            "setblock 16099 63 16100 oak_stairs[facing=east,half=bottom]",
            "setblock 16100 63 16100 oak_stairs[facing=east,half=bottom]",
            "execute if block 16100 63 16100 oak_stairs[shape=straight] run setblock 16101 63 16100 gold_block",
            "execute if block 16100 63 16100 oak_stairs[shape=inner_left] run setblock 16101 63 16100 diamond_block",
            "execute if block 16100 63 16100 oak_stairs[shape=inner_right] run setblock 16101 63 16100 emerald_block",
            "setblock 16100 63 16100 air",
            "setblock 16099 63 16100 air",
            "setblock 16101 63 16100 air",
            "forceload remove 16100 16100",
        ),
        "command_outcome",
        (
            "net/minecraft/server/commands/SetBlockCommand.java",
            "net/minecraft/commands/arguments/blocks/BlockInput.java",
            "net/minecraft/world/level/block/StairBlock.java",
        ),
        (
            "forceload:ok",
            "command:ok",
            "setblock:changed",
            "setblock:changed",
            "setblock:changed",
            "execute:matched",
            "execute:not_matched",
            "execute:not_matched",
            "setblock:changed",
            "setblock:changed",
            "setblock:changed",
            "forceload:ok",
        ),
    ),
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--vanilla", default="127.0.0.1:25576")
    parser.add_argument("--pumpkin", default="127.0.0.1:25575")
    parser.add_argument("--vanilla-password", default="parity")
    parser.add_argument("--pumpkin-password", default="pumpkintest")
    parser.add_argument("--vanilla-source", type=Path, default=Path(os.environ.get("VANILLA_DECOMPILE", "/home/eshanki/pumpkin-vanilla-26.2/decompiled")))
    parser.add_argument("--out", type=Path, default=ROOT / "target/differential/latest.json")
    parser.add_argument("--expected-version", default="26.2")
    parser.add_argument(
        "--allow-destructive",
        action="store_true",
        help="allow probes that mutate weather, difficulty, blocks, or force-load state",
    )
    args = parser.parse_args()

    if not args.allow_destructive:
        parser.error("differential probes are destructive; pass --allow-destructive for disposable endpoints")

    try:
        source_files = _source_files(args.vanilla_source)
    except FileNotFoundError as error:
        parser.error(f"vanilla source file is missing: {error.filename}")

    targets = (
        _parse_target("vanilla", args.vanilla, args.vanilla_password),
        _parse_target("pumpkin", args.pumpkin, args.pumpkin_password),
    )
    if not args.vanilla_source.is_dir():
        parser.error(f"vanilla source directory does not exist: {args.vanilla_source}")
    started = time.time()
    observations: dict[str, dict[str, list[dict[str, str]]]] = {}
    errors: dict[str, str] = {}
    for target in targets:
        try:
            observations[target.name] = _run_target(target, args.expected_version)
        except (OSError, RconError) as error:
            errors[target.name] = str(error)

    result = {
        "schema": 1,
        "started_at_unix": started,
        "source_root": str(args.vanilla_source),
        "source_files": source_files,
        "source_manifest_sha256": hashlib.sha256(
            json.dumps(source_files, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest(),
        "expected_version": args.expected_version,
        "probes": [probe.__dict__ for probe in PROBES],
        "observations": observations,
        "errors": errors,
    }
    result["comparisons"] = _compare(observations)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result["comparisons"], indent=2, sort_keys=True))
    return 0 if not errors and all(item["equal"] for item in result["comparisons"]) else 1


def _run_target(target: Target, expected_version: str) -> dict[str, list[dict[str, str]]]:
    with Rcon(target.host, target.port, target.password) as rcon:
        identity = rcon.command("version")
        if not re.search(rf"(?<![0-9]){re.escape(expected_version)}(?![0-9])", identity, re.IGNORECASE):
            raise RconError(f"{target.name} version did not contain {expected_version!r}: {identity!r}")

        observations = {
            "__identity__": [
                {
                    "command": "version",
                    "raw": identity,
                    "normalized": identity.strip(),
                    "sha256": hashlib.sha256(identity.encode("utf-8")).hexdigest(),
                }
            ]
        }
        for probe in PROBES:
            observations[probe.name] = _run_probe(rcon, probe)
        return observations


def _run_probe(rcon: Rcon, probe: Probe) -> list[dict[str, str]]:
    try:
        values = [
            {
                "command": command,
                "raw": (raw := rcon.command(command)),
                "normalized": _normalize(raw, probe.normalizer, command),
            }
            for command in probe.commands
        ]
    except Exception as error:
        cleanup_errors = _run_cleanup(rcon, probe.name)
        if cleanup_errors:
            raise RconError(f"{error}; cleanup failed: {'; '.join(cleanup_errors)}") from error
        raise

    cleanup_errors = _run_cleanup(rcon, probe.name)
    if cleanup_errors:
        raise RconError(f"cleanup failed for {probe.name}: {'; '.join(cleanup_errors)}")
    return values


def _run_cleanup(rcon: Rcon, probe_name: str) -> list[str]:
    errors = []
    for cleanup in _cleanup_commands(probe_name):
        try:
            raw = rcon.command(cleanup)
            if not _cleanup_succeeded(cleanup, raw) and not _cleanup_noop_is_safe(rcon, cleanup, raw):
                errors.append(f"{cleanup}: unexpected response {raw!r}")
        except (OSError, RconError) as error:
            errors.append(f"{cleanup}: {error}")
    return errors


def _cleanup_succeeded(command: str, value: str) -> bool:
    lower = value.lower()
    if _has_error_marker(lower):
        return False
    if command == "weather clear":
        return "set the weather to clear" in lower
    if command.startswith("difficulty "):
        difficulty = command.split()[1]
        return (
            f"the difficulty has been set to {difficulty}" in lower
            or f"the difficulty did not change; it is already set to {difficulty}" in lower
        )
    if command == "gamerule spawn_mobs true":
        return "now set to: true" in lower
    if command == "forceload remove all":
        return "unmarked all force loaded chunks" in lower
    if command.startswith("forceload add "):
        return "to be force loaded" in lower and "no chunks were marked" not in lower
    if command.startswith("forceload remove "):
        return "for force loading" in lower and "no chunks were removed" not in lower
    if command.startswith("setblock "):
        return "changed the block at" in lower
    return True


def _cleanup_noop_is_safe(rcon: Rcon, command: str, value: str) -> bool:
    if value.strip().lower() != "could not set the block":
        return False
    match = re.fullmatch(r"setblock (-?\d+) (-?\d+) (-?\d+) air", command)
    if not match:
        return False
    x, y, z = match.groups()
    verification = rcon.command(f"execute if block {x} {y} {z} minecraft:air run say differential_cleanup_ok")
    return "differential_cleanup_ok" in verification.lower() and not _has_error_marker(verification.lower())


def _cleanup_commands(probe_name: str) -> tuple[str, ...]:
    if probe_name == "command_semantics":
        return ("weather clear", "difficulty normal")
    if probe_name == "time_and_gamerule_queries":
        return ("gamerule spawn_mobs true",)
    if probe_name in {"forceload_block_mutation", "exact_rcon_command_text"}:
        return ("forceload remove all",)
    if probe_name == "execute_unloaded_block_predicate":
        return (
            "setblock 0 63 0 air",
            "forceload remove all",
        )
    if probe_name == "setblock_defined_properties_and_shapes":
        return (
            "forceload add 16100 16100",
            "setblock 16099 63 16100 air",
            "setblock 16100 63 16100 air",
            "setblock 16101 63 16100 air",
            "forceload remove all",
        )
    return ()


def _compare(observations: dict[str, dict[str, list[dict[str, str]]]]) -> list[dict[str, object]]:
    if "vanilla" not in observations or "pumpkin" not in observations:
        return []
    result = []
    for probe in PROBES:
        vanilla = observations["vanilla"].get(probe.name, [])
        pumpkin = observations["pumpkin"].get(probe.name, [])
        vanilla_normalized = [item["normalized"] for item in vanilla]
        pumpkin_normalized = [item["normalized"] for item in pumpkin]
        vanilla_expected = _meets_expectations(probe, vanilla_normalized)
        pumpkin_expected = _meets_expectations(probe, pumpkin_normalized)
        result.append(
            {
                "probe": probe.name,
                "equal": vanilla_normalized == pumpkin_normalized and vanilla_expected and pumpkin_expected,
                "expected": list(probe.expected),
                "vanilla_expected": vanilla_expected,
                "pumpkin_expected": pumpkin_expected,
                "vanilla": vanilla_normalized,
                "pumpkin": pumpkin_normalized,
            }
        )
    return result


def _meets_expectations(probe: Probe, values: list[str]) -> bool:
    if probe.expected:
        return values == list(probe.expected)
    if probe.normalizer == "exact_output":
        return _exact_output_meets_expectations(probe, values)
    return all(value not in {"weather:error", "difficulty:error", "command:error"} for value in values)


def _exact_output_meets_expectations(probe: Probe, values: list[str]) -> bool:
    if probe.name != "exact_rcon_command_text" or len(values) != 4:
        return bool(values)
    expected = (
        r"unmarked all force loaded chunks in minecraft:overworld",
        r"marked chunk \[1000, 1000\] in minecraft:overworld to be force loaded",
        r"chunk at \[1000, 1000\] in minecraft:overworld is marked for force loading",
        r"unmarked chunk \[1000, 1000\] in minecraft:overworld for force loading",
    )
    return all(re.fullmatch(pattern, value.strip().lower()) for pattern, value in zip(expected, values, strict=True))


def _normalize(value: str, kind: str, command: str) -> str:
    value = re.sub(r"\d+ms", "<time>", value)
    expected_chunk = _forceload_chunk(command)
    if command == "time query gametime":
        return "gametime:" + ("ok" if re.search(r"\b(?:is|time is) \d+", value) else "error")
    if command == "gamerule spawn_mobs":
        match = re.search(r"currently set to:\s*(true|false)", value, flags=re.IGNORECASE)
        return "spawn_mobs:" + (match.group(1).lower() if match else "error")
    if command == "gamerule spawn_mobs true":
        lower = value.lower()
        return "spawn_mobs:set:true" if "now set to: true" in lower else "spawn_mobs:error"
    if kind == "unloaded_predicate" and command == "forceload remove all":
        return "forceload:ok" if "unmarked all force loaded chunks" in value.lower() else "forceload:error"
    if kind in {"exact_output", "unloaded_predicate"}:
        return value
    if command == "weather rain":
        lower = value.lower()
        return "weather:rain" if lower.strip() == "set the weather to rain" else "weather:error"
    if command == "weather clear":
        lower = value.lower()
        return "weather:clear" if lower.strip() == "set the weather to clear" else "weather:error"
    if command.startswith("difficulty "):
        difficulty = command.split()[1]
        lower = value.lower()
        if "did not change" in lower or "already" in lower:
            return f"difficulty:{difficulty}:unchanged"
        return f"difficulty:{difficulty}:set" if "set" in lower and difficulty in lower and not _has_error_marker(lower) else "difficulty:error"
    if command.startswith("forceload query "):
        lower = value.lower()
        if _has_error_marker(lower) or "not marked for force loading" in lower:
            return "forceload:query_error"
        return "forceload:query_ok" if expected_chunk in value else "forceload:query_error"
    if command == "forceload remove all":
        lower = value.lower()
        return "forceload:ok" if not _has_error_marker(lower) and "unmarked all force loaded chunks" in lower else "forceload:error"
    if command.startswith("forceload add "):
        lower = value.lower()
        return "forceload:ok" if not _has_error_marker(lower) and "marked" in lower and expected_chunk in value and "no chunks" not in lower else "forceload:error"
    if command.startswith("forceload remove "):
        lower = value.lower()
        return "forceload:ok" if not _has_error_marker(lower) and "unmarked" in lower and expected_chunk in value and "no chunks" not in lower else "forceload:error"
    if command.startswith("setblock "):
        lower = value.lower()
        if "not loaded" in lower:
            return "setblock:not_loaded"
        if _has_error_marker(lower):
            if "could not set the block" in lower:
                return "setblock:failed"
            return "setblock:error"
        if "changed the block" in lower:
            return "setblock:changed"
        return "setblock:error"
    if command.startswith("fill "):
        lower = value.lower()
        if _has_error_marker(lower):
            return "command:error"
        return "command:ok" if "successfully filled" in lower else "command:error"
    if command.endswith("run setblock 16001 63 16000 gold_block"):
        lower = value.lower()
        if _has_error_marker(lower):
            if any(marker in lower for marker in ("condition", "failed", "did not")):
                return "execute:condition_failed"
            return "execute:error"
        if "changed the block" in lower:
            return "execute:unless_air" if command.startswith("execute unless") else "execute:stone"
        return "execute:error"
    if command.startswith("execute ") and " run setblock 16101 63 16100 " in command:
        lower = value.lower()
        if re.fullmatch(r"test failed(?:\. count: \d+)?", lower.strip()):
            return "execute:not_matched"
        if "changed the block" in lower:
            return "execute:matched"
        return "execute:error"
    if kind == "lines":
        return "\n".join(line.strip() for line in value.splitlines() if line.strip())
    if kind == "command_outcome":
        lower = value.lower()
        if any(marker in lower for marker in ("error", "unknown", "incorrect", "can't", "cannot")):
            return "command:error"
        return (
            "command:ok"
            if any(marker in lower for marker in ("changed the block", "filled", "set the difficulty", "set the weather"))
            else "command:error"
        )
    raise ValueError(f"unknown normalizer {kind}")


def _has_error_marker(value: str) -> bool:
    return any(marker in value for marker in ("error", "unknown", "incorrect", "can't", "cannot", "could not", "failed"))


def _forceload_chunk(command: str) -> str:
    match = re.match(r"forceload (?:add|query|remove) (-?\d+) (-?\d+)$", command)
    if not match:
        return "[]"
    x, z = (int(value) for value in match.groups())
    return f"[{x // 16}, {z // 16}]"


def _source_digest(path: Path) -> dict[str, str | int]:
    data = path.read_bytes()
    return {"path": str(path), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}


def _source_files(root: Path) -> dict[str, dict[str, str | int]]:
    return {
        probe.name: [_source_digest(root / source) for source in probe.source]
        for probe in PROBES
    }


def _parse_target(name: str, address: str, password: str) -> Target:
    host, port = address.rsplit(":", 1)
    return Target(name, host, int(port), password)


if __name__ == "__main__":
    sys.exit(main())
