# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""State journal, status inspection, grant reconciliation, and configuration rollback."""

from __future__ import annotations

import dataclasses
import datetime
import json
import os
import pathlib
import shutil
import sys
from collections.abc import Mapping, Sequence
from typing import Any


def get_default_state_dir() -> pathlib.Path:
    """Resolve default state directory adhering to Fuchsia XDG standards."""
    state_home = os.environ.get("XDG_STATE_HOME") or os.environ.get(
        "XDG_DATA_HOME"
    )
    if state_home:
        return pathlib.Path(state_home) / "Fuchsia" / "agents" / "setup"

    return (
        pathlib.Path.home()
        / ".local"
        / "share"
        / "Fuchsia"
        / "agents"
        / "setup"
    )


MAX_BACKUPS = 10


@dataclasses.dataclass
class StateJournal:
    """State journal tracking agent setup history and managed permissions."""

    schema_version: int = 1
    last_updated: str = ""
    fuchsia_root: str = ""
    active_profile: str = ""
    managed_grants: dict[str, list[str]] = dataclasses.field(
        default_factory=lambda: {"allow": [], "deny": [], "ask": []}
    )
    history: list[dict[str, Any]] = dataclasses.field(default_factory=list)


def load_state(state_path: pathlib.Path) -> StateJournal:
    """Load state from JSON; if missing/corrupt, returns clean default StateJournal."""
    if not state_path.is_file():
        return StateJournal()
    try:
        with state_path.open("r", encoding="utf-8") as file_handle:
            data = json.load(file_handle)
        if not isinstance(data, dict):
            return StateJournal()
        managed_raw = data.get("managed_grants", {})
        managed_grants = {
            cat: list(managed_raw.get(cat, []))
            for cat in ("allow", "deny", "ask")
        }
        return StateJournal(
            schema_version=data.get("schema_version", 1),
            last_updated=data.get("last_updated", ""),
            fuchsia_root=data.get("fuchsia_root", ""),
            active_profile=data.get("active_profile", ""),
            managed_grants=managed_grants,
            history=list(data.get("history", [])),
        )
    except Exception:
        return StateJournal()


def save_state(state: StateJournal, state_path: pathlib.Path) -> None:
    """Atomically save state via .tmp file and replace. Creates parent directories as needed."""
    state_path.parent.mkdir(parents=True, exist_ok=True)
    temp_path = state_path.with_name(f".{state_path.name}.tmp")
    data = dataclasses.asdict(state)
    with temp_path.open("w", encoding="utf-8") as file_handle:
        json.dump(data, file_handle, indent=2)
        file_handle.write("\n")
    temp_path.replace(state_path)


def create_backup(
    config_path: pathlib.Path,
    backups_dir: pathlib.Path,
    max_backups: int = MAX_BACKUPS,
) -> pathlib.Path | None:
    """Copy config_path to timestamped file in backups_dir, prune oldest backups beyond max_backups, and return backup path."""
    if not config_path.is_file():
        return None
    backups_dir.mkdir(parents=True, exist_ok=True)
    now = datetime.datetime.now()
    timestamp = now.strftime("%Y%m%d_%H%M%S")

    same_second = list(backups_dir.glob(f"config_{timestamp}*.json"))
    if not same_second:
        backup_path = backups_dir / f"config_{timestamp}.json"
    else:
        counter = len(same_second)
        while True:
            candidate = backups_dir / f"config_{timestamp}_{counter:03d}.json"
            if not candidate.exists():
                backup_path = candidate
                break
            counter += 1

    shutil.copy2(config_path, backup_path)

    # Prune oldest backups beyond max_backups
    existing_backups = sorted(
        [p for p in backups_dir.glob("config_*.json") if p.is_file()],
        key=lambda p: (p.stat().st_mtime_ns, p.name),
    )
    if len(existing_backups) > max_backups:
        for old_file in existing_backups[:-max_backups]:
            try:
                old_file.unlink(missing_ok=True)
            except OSError:
                pass
    return backup_path


def reconcile_grants(
    existing_grants: Mapping[str, Sequence[str]],
    prev_managed: Mapping[str, Sequence[str]],
    target_managed: Mapping[str, Sequence[str]],
) -> tuple[dict[str, list[str]], dict[str, list[str]]]:
    """Reconcile existing grants with previously managed and newly targeted grants.

    - Computes user custom rules: U[cat] = [r for r in existing_grants.get(cat, []) if r not in prev_managed.get(cat, [])]
    - Resolves opposing category conflicts (so moving a rule from deny -> allow or vice versa cleans up properly).
    - Final grants: final[cat] = U[cat] + [r for r in target_managed.get(cat, []) if r not in U[cat]]
    - Returns (final_grants, target_managed)
    """
    categories = ("allow", "deny", "ask")
    normalized_target: dict[str, list[str]] = {
        cat: list(dict.fromkeys(target_managed.get(cat, [])))
        for cat in categories
    }
    prev_managed_sets = {
        cat: set(prev_managed.get(cat, [])) for cat in categories
    }

    # Compute user custom rules U[cat]
    user_custom: dict[str, list[str]] = {
        cat: [
            r
            for r in existing_grants.get(cat, [])
            if r not in prev_managed_sets[cat]
        ]
        for cat in categories
    }

    # Resolve opposing category conflicts:
    # If a rule r is in target_managed[cat], remove it from user_custom[other_cat]
    target_sets = {cat: set(normalized_target[cat]) for cat in categories}
    opposing_target_rules = {
        cat: set().union(
            *(target_sets[other] for other in categories if other != cat)
        )
        for cat in categories
    }
    for cat in categories:
        if opposing_target_rules[cat]:
            user_custom[cat] = [
                r
                for r in user_custom[cat]
                if r not in opposing_target_rules[cat]
            ]

    # Compute final grants
    final_grants: dict[str, list[str]] = {}
    for cat in categories:
        cat_custom_set = set(user_custom[cat])
        cat_rules = list(user_custom[cat])
        for r in normalized_target[cat]:
            if r not in cat_custom_set:
                cat_rules.append(r)
                cat_custom_set.add(r)
        final_grants[cat] = cat_rules

    return final_grants, normalized_target


def rollback(
    config_path: pathlib.Path,
    state_path: pathlib.Path,
    backups_dir: pathlib.Path,
    steps: int = 1,
    dry_run: bool = False,
) -> bool:
    """Rolls back state and config to `steps` transactions ago using recorded backup file and history."""
    if steps < 1:
        print(
            f"Error: Rollback steps must be positive (got {steps}).",
            file=sys.stderr,
        )
        return False

    state = load_state(state_path)
    if not state.history:
        print(
            "Error: No transaction history available for rollback.",
            file=sys.stderr,
        )
        return False

    if steps > len(state.history):
        print(
            f"Error: Cannot rollback {steps} steps; only {len(state.history)} transactions in history.",
            file=sys.stderr,
        )
        return False

    target_entry = state.history[-steps]
    backup_file_str = target_entry.get("backup_file")
    backup_file: pathlib.Path | None = None
    if backup_file_str:
        candidate = pathlib.Path(backup_file_str)
        if candidate.is_file():
            backup_file = candidate
        elif (backups_dir / candidate.name).is_file():
            backup_file = backups_dir / candidate.name
        else:
            print(
                f"Error: Backup file not found: {backup_file_str}",
                file=sys.stderr,
            )
            return False

    remaining_history = state.history[:-steps]
    if remaining_history:
        prev_entry = remaining_history[-1]
        new_profile = prev_entry.get("profile", "")
        new_managed = {
            cat: list(prev_entry.get("managed_grants", {}).get(cat, []))
            for cat in ("allow", "deny", "ask")
        }
        new_timestamp = prev_entry.get("timestamp", "")
    else:
        new_profile = ""
        new_managed = {"allow": [], "deny": [], "ask": []}
        new_timestamp = ""

    if dry_run:
        print(f"\n[DRY RUN] Would roll back {steps} transaction(s).")
        if backup_file:
            print(f"[DRY RUN] Would restore config from backup: {backup_file}")
        else:
            print(f"[DRY RUN] Would remove config file: {config_path}")
        print(
            f"[DRY RUN] Would restore active profile to: {new_profile or '(none)'}"
        )
        print(f"[DRY RUN] Would update state journal at: {state_path}")
        return True

    if backup_file:
        config_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(backup_file, config_path)
        print(f"\nRestored configuration from backup: {backup_file}")
    else:
        if config_path.is_file():
            config_path.unlink()
            print(f"\nRemoved configuration file: {config_path}")

    state.active_profile = new_profile
    state.managed_grants = new_managed
    state.last_updated = new_timestamp
    state.history = remaining_history
    save_state(state, state_path)

    print(
        f"Successfully rolled back {steps} step(s). Active profile: {new_profile or '(none)'}"
    )
    return True


def reset(
    config_path: pathlib.Path,
    state_path: pathlib.Path,
    backups_dir: pathlib.Path,
    dry_run: bool = False,
) -> bool:
    """Removes all managed_grants from config.json, keeping all user custom rules and non-permission userSettings."""
    state = load_state(state_path)
    config_data: dict[str, Any] = {}

    if config_path.is_file():
        try:
            with config_path.open("r", encoding="utf-8") as fh:
                loaded = json.load(fh)
                if isinstance(loaded, dict):
                    config_data = loaded
        except Exception as error:
            print(
                f"Error reading JSON from {config_path}: {error}",
                file=sys.stderr,
            )
            return False

        user_settings = config_data.setdefault("userSettings", {})
        grants = user_settings.setdefault("globalPermissionGrants", {})
        managed = state.managed_grants
        for cat in ("allow", "deny", "ask"):
            if cat in grants:
                managed_set = set(managed.get(cat, []))
                grants[cat] = [
                    r for r in grants.get(cat, []) if r not in managed_set
                ]

    if dry_run:
        print(
            f"\n[DRY RUN] Would purge Fuchsia-managed rules from: {config_path}"
        )
        print(f"[DRY RUN] Would reset state journal at: {state_path}")
        return True

    if config_path.is_file():
        create_backup(config_path, backups_dir)
        temp_config = config_path.with_name(f".{config_path.name}.tmp")
        with temp_config.open("w", encoding="utf-8") as fh:
            json.dump(config_data, fh, indent=2)
            fh.write("\n")
        temp_config.replace(config_path)
        print(f"\nPurged Fuchsia-managed rules from: {config_path}")

    clean_state = StateJournal(fuchsia_root=state.fuchsia_root)
    save_state(clean_state, state_path)
    print(f"Successfully reset state journal at: {state_path}")
    return True


def format_status(config_path: pathlib.Path, state_path: pathlib.Path) -> str:
    """Formats human-readable status showing active profile, managed vs user custom rule counts per category, and recent backup history."""
    state = load_state(state_path)
    config_grants: dict[str, list[str]] = {}
    config_exists = config_path.is_file()

    if config_exists:
        try:
            with config_path.open("r", encoding="utf-8") as fh:
                data = json.load(fh)
                if isinstance(data, dict):
                    config_grants = (
                        data.get("userSettings", {}).get(
                            "globalPermissionGrants", {}
                        )
                        or {}
                    )
        except Exception:
            pass

    lines: list[str] = [
        "=== AI Coding Agent Configuration Status ===",
        f"Active Profile : {state.active_profile or '(none)'}",
        f"Last Updated   : {state.last_updated or '(never)'}",
        f"Fuchsia Root   : {state.fuchsia_root or '(not set)'}",
        f"Config File    : {config_path} {'[exists]' if config_exists else '[not found]'}",
        f"State File     : {state_path} {'[exists]' if state_path.is_file() else '[not found]'}",
        "",
        "Rule Breakdown:",
    ]

    categories = ("allow", "deny", "ask")
    for cat in categories:
        total = config_grants.get(cat, [])
        managed = state.managed_grants.get(cat, [])
        managed_count = len([r for r in total if r in managed])
        custom_count = len([r for r in total if r not in managed])
        lines.append(
            f"  [{cat.upper():<5}] : {len(total):>3} total ({managed_count:>3} managed, {custom_count:>3} custom)"
        )

    lines.append("")
    lines.append(f"Recent History ({len(state.history)} transactions):")
    if not state.history:
        lines.append("  (no history recorded)")
    else:
        for idx, entry in enumerate(reversed(state.history[-5:]), start=1):
            ts = entry.get("timestamp", "unknown")
            prof = entry.get("profile", "unknown")
            backup = entry.get("backup_file") or "(none)"
            lines.append(f"  {idx}. [{ts}] Profile: {prof} | Backup: {backup}")

    return "\n".join(lines)
