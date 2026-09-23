# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for agent state journal, status formatting, and configuration rollback."""

from __future__ import annotations

import json
import pathlib
import unittest
from collections.abc import Sequence

from agents.lib import state
from agents_testing.base import BaseTestCase


class StateTest(BaseTestCase):
    """Hermetic unit tests for state journal and rollback operations."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_root = self.test_dir
        self.state_dir = self.mock_root / "state"
        self.state_path = self.state_dir / "state.json"
        self.backups_dir = self.state_dir / "backups"
        self.config_path = self.mock_root / "config.json"

    def test_get_default_state_dir_fallback(self) -> None:
        """Verify default fallback to ~/.local/share/Fuchsia/agents/setup when no env vars are set."""
        self.patch_environ(clear=True)
        self.patch_object(
            pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
        )
        expected = pathlib.Path("/mock/home/.local/share/Fuchsia/agents/setup")
        self.assertEqual(state.get_default_state_dir(), expected)

    def test_get_default_state_dir_xdg_state_home(self) -> None:
        """Verify XDG_STATE_HOME environment variable override."""
        self.patch_environ(clear=True, XDG_STATE_HOME="/custom/xdg_state")
        expected = pathlib.Path("/custom/xdg_state/Fuchsia/agents/setup")
        self.assertEqual(state.get_default_state_dir(), expected)

    def test_get_default_state_dir_xdg_data_home(self) -> None:
        """Verify XDG_DATA_HOME environment variable fallback when XDG_STATE_HOME is not set."""
        self.patch_environ(clear=True, XDG_DATA_HOME="/custom/xdg_data")
        expected = pathlib.Path("/custom/xdg_data/Fuchsia/agents/setup")
        self.assertEqual(state.get_default_state_dir(), expected)

    def test_get_default_state_dir_precedence(self) -> None:
        """Verify XDG_STATE_HOME takes precedence over XDG_DATA_HOME."""
        self.patch_environ(
            clear=True,
            XDG_STATE_HOME="/custom/xdg_state",
            XDG_DATA_HOME="/custom/xdg_data",
        )
        expected = pathlib.Path("/custom/xdg_state/Fuchsia/agents/setup")
        self.assertEqual(state.get_default_state_dir(), expected)

    def test_load_and_save_state(self) -> None:
        """Verify saving and loading StateJournal round-trips correctly."""
        journal = state.StateJournal(
            schema_version=1,
            last_updated="2026-08-26 12:00:00",
            fuchsia_root="/path/to/fuchsia",
            active_profile="local-changes",
            managed_grants={
                "allow": ["command(fx status)"],
                "deny": ["command(git clean)"],
                "ask": ["command(fx reboot)"],
            },
            history=[
                {
                    "timestamp": "2026-08-26 12:00:00",
                    "profile": "local-changes",
                    "backup_file": "/path/to/backup.json",
                    "managed_grants": {
                        "allow": ["command(fx status)"],
                        "deny": ["command(git clean)"],
                        "ask": ["command(fx reboot)"],
                    },
                }
            ],
        )

        state.save_state(journal, self.state_path)
        self.assertTrue(self.state_path.is_file())

        loaded = state.load_state(self.state_path)
        self.assertEqual(loaded.schema_version, 1)
        self.assertEqual(loaded.last_updated, "2026-08-26 12:00:00")
        self.assertEqual(loaded.fuchsia_root, "/path/to/fuchsia")
        self.assertEqual(loaded.active_profile, "local-changes")
        self.assertEqual(loaded.managed_grants["allow"], ["command(fx status)"])
        self.assertEqual(loaded.managed_grants["deny"], ["command(git clean)"])
        self.assertEqual(loaded.managed_grants["ask"], ["command(fx reboot)"])
        self.assertEqual(len(loaded.history), 1)
        self.assertEqual(loaded.history[0]["profile"], "local-changes")

    def test_load_state_missing_and_corrupt(self) -> None:
        """Verify load_state returns a clean default journal when missing or corrupted."""
        # Missing file
        missing_path = self.state_dir / "non_existent.json"
        clean_state = state.load_state(missing_path)
        self.assertIsInstance(clean_state, state.StateJournal)
        self.assertEqual(clean_state.active_profile, "")
        self.assertEqual(clean_state.history, [])

        # Corrupt JSON
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self.state_path.write_text("{corrupt json", encoding="utf-8")
        clean_state = state.load_state(self.state_path)
        self.assertIsInstance(clean_state, state.StateJournal)
        self.assertEqual(clean_state.active_profile, "")

        # Non-dict JSON
        self.state_path.write_text("[1, 2, 3]", encoding="utf-8")
        clean_state = state.load_state(self.state_path)
        self.assertIsInstance(clean_state, state.StateJournal)
        self.assertEqual(clean_state.active_profile, "")

    def test_create_backup_and_pruning(self) -> None:
        """Verify backup creation and automatic pruning beyond max_backups."""
        # Non-existent config
        self.assertIsNone(
            state.create_backup(self.config_path, self.backups_dir)
        )

        # Create config file
        self.config_path.write_text('{"test": 1}', encoding="utf-8")

        # Create backups with small max_backups limit
        max_backups = 3
        created: list[pathlib.Path] = []
        for i in range(5):
            self.config_path.write_text(f'{{"test": {i}}}', encoding="utf-8")
            b_path = state.create_backup(
                self.config_path, self.backups_dir, max_backups=max_backups
            )
            self.assertIsNotNone(b_path)
            assert b_path is not None
            created.append(b_path)

        existing_backups = list(self.backups_dir.glob("config_*.json"))
        self.assertEqual(len(existing_backups), max_backups)
        # Oldest backup should have been pruned
        self.assertFalse(created[0].exists())
        self.assertFalse(created[1].exists())
        self.assertTrue(created[-1].exists())

    def test_reconcile_grants_profile_switching(self) -> None:
        """Verify switching profiles moves rules across categories (deny -> allow)."""
        existing_grants: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)"],
            "deny": ["command(fx format-code)", "command(git clean)"],
            "ask": ["command(fx ota)"],
        }
        prev_managed: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)"],
            "deny": ["command(fx format-code)", "command(git clean)"],
            "ask": ["command(fx ota)"],
        }
        target_managed: dict[str, Sequence[str]] = {
            "allow": [
                "command(fx status)",
                "command(fx format-code)",
                "command(fx ota)",
            ],
            "deny": ["command(git clean)"],
            "ask": [],
        }

        final_grants, target = state.reconcile_grants(
            existing_grants=existing_grants,
            prev_managed=prev_managed,
            target_managed=target_managed,
        )

        self.assertEqual(
            final_grants["allow"],
            [
                "command(fx status)",
                "command(fx format-code)",
                "command(fx ota)",
            ],
        )
        self.assertEqual(final_grants["deny"], ["command(git clean)"])
        self.assertEqual(final_grants["ask"], [])
        self.assertNotIn("command(fx format-code)", final_grants["deny"])
        self.assertNotIn("command(fx ota)", final_grants["ask"])
        self.assertEqual(target, target_managed)

    def test_reconcile_grants_preserves_user_custom(self) -> None:
        """Verify user-added custom rules are preserved during reconciliation."""
        existing_grants: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)", "command(my_custom_tool)"],
            "deny": ["command(git clean)", "command(custom_block)"],
            "ask": ["command(custom_prompt)"],
        }
        prev_managed: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)"],
            "deny": ["command(git clean)"],
            "ask": [],
        }
        target_managed: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)", "command(fx test)"],
            "deny": ["command(git clean)"],
            "ask": [],
        }

        final_grants, _ = state.reconcile_grants(
            existing_grants=existing_grants,
            prev_managed=prev_managed,
            target_managed=target_managed,
        )

        self.assertIn("command(my_custom_tool)", final_grants["allow"])
        self.assertIn("command(fx test)", final_grants["allow"])
        self.assertIn("command(custom_block)", final_grants["deny"])
        self.assertIn("command(custom_prompt)", final_grants["ask"])

    def test_reconcile_grants_list_evolution(self) -> None:
        """Verify obsolete managed rules removed in upstream lists are purged."""
        existing_grants: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)", "command(fx obsolete_cmd)"],
            "deny": ["command(git clean)"],
            "ask": [],
        }
        prev_managed: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)", "command(fx obsolete_cmd)"],
            "deny": ["command(git clean)"],
            "ask": [],
        }
        target_managed: dict[str, Sequence[str]] = {
            "allow": ["command(fx status)"],
            "deny": ["command(git clean)"],
            "ask": [],
        }

        final_grants, _ = state.reconcile_grants(
            existing_grants=existing_grants,
            prev_managed=prev_managed,
            target_managed=target_managed,
        )

        self.assertNotIn("command(fx obsolete_cmd)", final_grants["allow"])
        self.assertEqual(final_grants["allow"], ["command(fx status)"])

    def test_rollback_success_and_failures(self) -> None:
        """Verify rollback error handling, dry run, and successful restoration."""
        # Failure: steps < 1
        self.assertFalse(
            state.rollback(
                self.config_path, self.state_path, self.backups_dir, steps=0
            )
        )
        self.assertIn("must be positive", self.stderr)

        # Failure: no history
        self.assertFalse(
            state.rollback(
                self.config_path, self.state_path, self.backups_dir, steps=1
            )
        )
        self.assertIn("No transaction history", self.stderr)

        # Create 2 transactions in journal and backup files
        self.backups_dir.mkdir(parents=True, exist_ok=True)
        backup1 = self.backups_dir / "config_20260826_010000.json"
        backup1.write_text('{"version": 1}', encoding="utf-8")
        backup2 = self.backups_dir / "config_20260826_020000.json"
        backup2.write_text('{"version": 2}', encoding="utf-8")

        self.config_path.write_text('{"version": 3}', encoding="utf-8")

        journal = state.StateJournal(
            schema_version=1,
            last_updated="2026-08-26 03:00:00",
            active_profile="external-changes",
            managed_grants={"allow": ["cmd3"], "deny": [], "ask": []},
            history=[
                {
                    "timestamp": "2026-08-26 01:00:00",
                    "profile": "read-only",
                    "backup_file": str(backup1),
                    "managed_grants": {
                        "allow": ["cmd1"],
                        "deny": [],
                        "ask": [],
                    },
                },
                {
                    "timestamp": "2026-08-26 02:00:00",
                    "profile": "local-changes",
                    "backup_file": str(backup2),
                    "managed_grants": {
                        "allow": ["cmd2"],
                        "deny": [],
                        "ask": [],
                    },
                },
            ],
        )
        state.save_state(journal, self.state_path)

        # Failure: steps > history length
        self.assertFalse(
            state.rollback(
                self.config_path, self.state_path, self.backups_dir, steps=3
            )
        )
        self.assertIn("Cannot rollback 3 steps", self.stderr)

        # Dry run rollback 1 step
        self.assertTrue(
            state.rollback(
                self.config_path,
                self.state_path,
                self.backups_dir,
                steps=1,
                dry_run=True,
            )
        )
        # Content unchanged after dry run
        self.assertEqual(
            self.config_path.read_text(encoding="utf-8"), '{"version": 3}'
        )

        # Execute rollback 1 step
        self.assertTrue(
            state.rollback(
                self.config_path,
                self.state_path,
                self.backups_dir,
                steps=1,
                dry_run=False,
            )
        )
        self.assertEqual(
            self.config_path.read_text(encoding="utf-8"), '{"version": 2}'
        )

        reloaded = state.load_state(self.state_path)
        self.assertEqual(reloaded.active_profile, "read-only")
        self.assertEqual(len(reloaded.history), 1)

        # Execute rollback remaining step (back to before initial setup)
        self.assertTrue(
            state.rollback(
                self.config_path,
                self.state_path,
                self.backups_dir,
                steps=1,
                dry_run=False,
            )
        )
        self.assertEqual(
            self.config_path.read_text(encoding="utf-8"), '{"version": 1}'
        )
        reloaded_final = state.load_state(self.state_path)
        self.assertEqual(reloaded_final.active_profile, "")
        self.assertEqual(len(reloaded_final.history), 0)

    def test_reset_success(self) -> None:
        """Verify reset removes managed grants while preserving custom rules and userSettings."""
        initial_config = {
            "rootField": "preserve",
            "userSettings": {
                "customConfig": 42,
                "globalPermissionGrants": {
                    "allow": ["command(fx status)", "command(my_custom_allow)"],
                    "deny": ["command(git clean)", "command(my_custom_deny)"],
                    "ask": ["command(my_custom_ask)"],
                },
            },
        }
        self.config_path.write_text(
            json.dumps(initial_config, indent=2), encoding="utf-8"
        )

        journal = state.StateJournal(
            schema_version=1,
            last_updated="2026-08-26 12:00:00",
            fuchsia_root="/path/to/fuchsia",
            active_profile="local-changes",
            managed_grants={
                "allow": ["command(fx status)"],
                "deny": ["command(git clean)"],
                "ask": [],
            },
            history=[{"timestamp": "2026-08-26 12:00:00"}],
        )
        state.save_state(journal, self.state_path)

        # Dry run reset
        self.assertTrue(
            state.reset(
                self.config_path,
                self.state_path,
                self.backups_dir,
                dry_run=True,
            )
        )
        # Verify nothing modified
        loaded_journal = state.load_state(self.state_path)
        self.assertEqual(loaded_journal.active_profile, "local-changes")

        # Execute reset
        self.assertTrue(
            state.reset(
                self.config_path,
                self.state_path,
                self.backups_dir,
                dry_run=False,
            )
        )

        # Check config.json
        with self.config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)

        self.assertEqual(data["rootField"], "preserve")
        self.assertEqual(data["userSettings"]["customConfig"], 42)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertEqual(grants["allow"], ["command(my_custom_allow)"])
        self.assertEqual(grants["deny"], ["command(my_custom_deny)"])
        self.assertEqual(grants["ask"], ["command(my_custom_ask)"])

        # Check state.json
        clean_state = state.load_state(self.state_path)
        self.assertEqual(clean_state.active_profile, "")
        self.assertEqual(clean_state.managed_grants["allow"], [])
        self.assertEqual(clean_state.history, [])

    def test_format_status(self) -> None:
        """Verify format_status displays active profile, rule breakdown, and history."""
        # Non-existent files
        non_existent_status = state.format_status(
            self.mock_root / "missing_config.json",
            self.mock_root / "missing_state.json",
        )
        self.assertIn("Active Profile : (none)", non_existent_status)
        self.assertIn("[not found]", non_existent_status)
        self.assertIn("(no history recorded)", non_existent_status)

        # Existing files with grants
        config_data = {
            "userSettings": {
                "globalPermissionGrants": {
                    "allow": ["command(fx status)", "command(my_tool)"],
                    "deny": ["command(git clean)"],
                    "ask": [],
                }
            }
        }
        self.config_path.write_text(
            json.dumps(config_data, indent=2), encoding="utf-8"
        )

        journal = state.StateJournal(
            schema_version=1,
            last_updated="2026-08-26 12:00:00",
            fuchsia_root="/path/to/fuchsia",
            active_profile="local-changes",
            managed_grants={
                "allow": ["command(fx status)"],
                "deny": ["command(git clean)"],
                "ask": [],
            },
            history=[
                {
                    "timestamp": "2026-08-26 12:00:00",
                    "profile": "local-changes",
                    "backup_file": "config_20260826_120000.json",
                }
            ],
        )
        state.save_state(journal, self.state_path)

        status_text = state.format_status(self.config_path, self.state_path)
        self.assertIn("Active Profile : local-changes", status_text)
        self.assertIn("Last Updated   : 2026-08-26 12:00:00", status_text)
        self.assertIn(
            "[ALLOW] :   2 total (  1 managed,   1 custom)", status_text
        )
        self.assertIn(
            "[DENY ] :   1 total (  1 managed,   0 custom)", status_text
        )
        self.assertIn("Recent History (1 transactions):", status_text)
        self.assertIn("Profile: local-changes", status_text)


if __name__ == "__main__":
    unittest.main()
