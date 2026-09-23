# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Configuration management and atomic writing for AI coding agents."""

from __future__ import annotations

import datetime
import json
import os
import pathlib
import sys
from collections.abc import Sequence
from typing import Any

from agents.lib import paths, state


def get_default_config_path() -> pathlib.Path:
    """Resolve default Gemini config path, respecting GEMINI_CONFIG_DIR or GEMINI_HOME."""
    if "GEMINI_CONFIG_DIR" in os.environ:
        return pathlib.Path(os.environ["GEMINI_CONFIG_DIR"]) / "config.json"
    if "GEMINI_HOME" in os.environ:
        return (
            pathlib.Path(os.environ["GEMINI_HOME"]) / "config" / "config.json"
        )
    return pathlib.Path.home() / ".gemini" / "config" / "config.json"


def load_config(config_path: pathlib.Path) -> dict[str, Any]:
    """Load and parse JSON configuration from config_path.

    Returns an empty dict if the file does not exist or does not contain a JSON object.
    """
    if not config_path.is_file():
        return {}
    data = json.loads(config_path.read_text(encoding="utf-8"))
    return data if isinstance(data, dict) else {}


def save_config_atomic(
    config_path: pathlib.Path, config_data: dict[str, Any]
) -> None:
    """Atomically write JSON configuration to disk via a temporary file."""
    config_path.parent.mkdir(parents=True, exist_ok=True)
    temp_config = config_path.with_name(f".{config_path.name}.tmp")
    temp_config.write_text(
        json.dumps(config_data, indent=2) + "\n", encoding="utf-8"
    )
    temp_config.replace(config_path)


def apply_grants(
    config_path: pathlib.Path | None = None,
    allow: Sequence[str] = (),
    deny: Sequence[str] = (),
    ask: Sequence[str] = (),
    dry_run: bool = False,
    state_dir: pathlib.Path | None = None,
    selected_profile: str = "",
) -> bool:
    """Load config.json, reconcile allow/deny/ask grants, create backup, and update state journal."""
    config_path = config_path or get_default_config_path()
    state_dir = state_dir or state.get_default_state_dir()
    state_path = state_dir / "state.json"
    backups_dir = state_dir / "backups"

    journal = state.load_state(state_path)
    try:
        config_data = load_config(config_path)
    except json.JSONDecodeError as error:
        print(
            f"Error reading JSON from {config_path}: {error}",
            file=sys.stderr,
        )
        return False

    user_settings = config_data.setdefault("userSettings", {})
    existing_grants = user_settings.setdefault("globalPermissionGrants", {})

    target_grants = {"allow": allow, "deny": deny, "ask": ask}
    final_grants, target_managed = state.reconcile_grants(
        existing_grants=existing_grants,
        prev_managed=journal.managed_grants,
        target_managed=target_grants,
    )

    print("\n=== Permission Updates Summary ===")
    total_changes = 0
    for category_name in ("allow", "deny", "ask"):
        new_rules = set(final_grants.get(category_name, []))
        old_rules = set(existing_grants.get(category_name, []))
        added = sorted(new_rules - old_rules)
        removed = sorted(old_rules - new_rules)
        total_changes += len(added) + len(removed)
        if added:
            print(f"  + Added to [{category_name}] ({len(added)}):")
            for grant in added:
                print(f"      - {grant}")
        if removed:
            print(f"  - Removed from [{category_name}] ({len(removed)}):")
            for grant in removed:
                print(f"      - {grant}")

    if total_changes == 0:
        print("  (No changes to rules; all entries up to date in config.json)")

    user_settings["globalPermissionGrants"] = final_grants

    if dry_run:
        print(f"\n[DRY RUN] Would write updated config to: {config_path}")
        print(f"[DRY RUN] Would update state journal at: {state_path}")
        if config_path.exists():
            print(f"[DRY RUN] Would create backup in: {backups_dir}")
        return True

    # Create backup before modifying existing config
    backup_path: pathlib.Path | None = None
    if config_path.exists():
        backup_path = state.create_backup(config_path, backups_dir)
        if backup_path:
            print(f"\nCreated backup of existing config at: {backup_path}")

    save_config_atomic(config_path, config_data)
    print(f"\nSuccessfully wrote updated config to: {config_path}")

    # Update state journal
    now_str = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    try:
        fuchsia_root_str = str(paths.find_fuchsia_dir())
    except RuntimeError:
        fuchsia_root_str = ""
    journal.schema_version = 1
    journal.last_updated = now_str
    journal.fuchsia_root = fuchsia_root_str
    journal.active_profile = selected_profile
    journal.managed_grants = target_managed
    journal.history.append(
        {
            "timestamp": now_str,
            "profile": selected_profile,
            "backup_file": str(backup_path) if backup_path else None,
            "managed_grants": target_managed,
        }
    )
    state.save_state(journal, state_path)

    return True
