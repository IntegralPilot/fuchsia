#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for config module, atomic writing, and backup creation."""

from __future__ import annotations

import json
import pathlib
import unittest

from agents.lib import config, paths, state
from agents_testing.base import BaseTestCase


class ConfigTest(BaseTestCase):
    """Hermetic unit tests for config operations."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_root = self.test_dir
        self.state_dir = self.mock_root / "state"
        self.fuchsia_dir = self.mock_root / "fuchsia"
        self.patch_object(
            paths, "find_fuchsia_dir", return_value=self.fuchsia_dir
        )

    def test_get_default_config_path_fallback(self) -> None:
        """Verify default fallback to ~/.gemini/config/config.json when no env vars are set."""
        self.patch_environ(clear=True)
        self.patch_object(
            pathlib.Path, "home", return_value=pathlib.Path("/mock/home")
        )
        expected = pathlib.Path("/mock/home/.gemini/config/config.json")
        self.assertEqual(config.get_default_config_path(), expected)

    def test_get_default_config_path_gemini_config_dir(self) -> None:
        """Verify GEMINI_CONFIG_DIR environment variable override."""
        self.patch_environ(clear=True, GEMINI_CONFIG_DIR="/custom/config/dir")
        expected = pathlib.Path("/custom/config/dir/config.json")
        self.assertEqual(config.get_default_config_path(), expected)

    def test_get_default_config_path_gemini_home(self) -> None:
        """Verify GEMINI_HOME environment variable override."""
        self.patch_environ(clear=True, GEMINI_HOME="/custom/gemini/home")
        expected = pathlib.Path("/custom/gemini/home/config/config.json")
        self.assertEqual(config.get_default_config_path(), expected)

    def test_get_default_config_path_precedence(self) -> None:
        """Verify GEMINI_CONFIG_DIR takes precedence over GEMINI_HOME."""
        self.patch_environ(
            clear=True,
            GEMINI_CONFIG_DIR="/custom/config/dir",
            GEMINI_HOME="/custom/gemini/home",
        )
        expected = pathlib.Path("/custom/config/dir/config.json")
        self.assertEqual(config.get_default_config_path(), expected)

    def test_load_config_nonexistent(self) -> None:
        """Verify load_config returns empty dict for nonexistent file."""
        nonexistent = self.mock_root / "does_not_exist.json"
        self.assertEqual(config.load_config(nonexistent), {})

    def test_load_config_valid(self) -> None:
        """Verify load_config correctly parses JSON object."""
        cfg_file = self.mock_root / "config.json"
        cfg_file.write_text('{"foo": "bar", "num": 42}', encoding="utf-8")
        self.assertEqual(
            config.load_config(cfg_file), {"foo": "bar", "num": 42}
        )

    def test_load_config_non_dict(self) -> None:
        """Verify load_config returns empty dict for JSON list or primitive."""
        cfg_file = self.mock_root / "config.json"
        cfg_file.write_text("[1, 2, 3]", encoding="utf-8")
        self.assertEqual(config.load_config(cfg_file), {})

    def test_load_config_invalid_json(self) -> None:
        """Verify load_config raises JSONDecodeError on syntax error."""
        cfg_file = self.mock_root / "config.json"
        cfg_file.write_text("{bad json", encoding="utf-8")
        with self.assertRaises(json.JSONDecodeError):
            config.load_config(cfg_file)

    def test_save_config_atomic(self) -> None:
        """Verify save_config_atomic writes valid JSON."""
        cfg_file = self.mock_root / "nested" / "config.json"
        data = {"userSettings": {"enabled": True}}
        config.save_config_atomic(cfg_file, data)
        self.assertTrue(cfg_file.is_file())
        with cfg_file.open("r", encoding="utf-8") as fh:
            loaded = json.load(fh)
        self.assertEqual(loaded, data)

    def test_apply_grants_new_file(self) -> None:
        """Verify apply_grants creates new config atomically and initializes state."""
        cfg_file = self.mock_root / "config.json"
        success = config.apply_grants(
            config_path=cfg_file,
            allow=["command(fx build)"],
            deny=["command(fx clean)"],
            ask=["command(fx reboot)"],
            dry_run=False,
            state_dir=self.state_dir,
            selected_profile="local-changes",
        )
        self.assertTrue(success)
        self.assertTrue(cfg_file.is_file())
        with cfg_file.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertEqual(grants["allow"], ["command(fx build)"])
        self.assertEqual(grants["deny"], ["command(fx clean)"])
        self.assertEqual(grants["ask"], ["command(fx reboot)"])

        journal = state.load_state(self.state_dir / "state.json")
        self.assertEqual(journal.active_profile, "local-changes")
        self.assertEqual(len(journal.history), 1)

    def test_apply_grants_existing_file_and_backup(self) -> None:
        """Verify preexisting config is backed up to backups/ before modification."""
        cfg_file = self.mock_root / "config.json"
        initial_data = {
            "userSettings": {
                "customSetting": 123,
                "globalPermissionGrants": {
                    "allow": ["command(custom_cmd)"],
                    "deny": [],
                    "ask": [],
                },
            },
            "rootField": "foo",
        }
        original_json = json.dumps(initial_data, indent=2)
        cfg_file.write_text(original_json, encoding="utf-8")

        success = config.apply_grants(
            config_path=cfg_file,
            allow=["command(fx build)"],
            deny=["command(fx clean)"],
            ask=[],
            dry_run=False,
            state_dir=self.state_dir,
            selected_profile="local-changes",
        )
        self.assertTrue(success)

        backups = list((self.state_dir / "backups").glob("config_*.json"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(encoding="utf-8"), original_json)

        data = json.loads(cfg_file.read_text(encoding="utf-8"))
        self.assertEqual(data["rootField"], "foo")
        self.assertEqual(data["userSettings"]["customSetting"], 123)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertEqual(
            grants["allow"], ["command(custom_cmd)", "command(fx build)"]
        )
        self.assertEqual(grants["deny"], ["command(fx clean)"])

    def test_apply_grants_dry_run(self) -> None:
        """Verify dry_run=True does not touch disk."""
        cfg_file = self.mock_root / "config.json"
        success = config.apply_grants(
            config_path=cfg_file,
            allow=["command(fx build)"],
            deny=[],
            ask=[],
            dry_run=True,
            state_dir=self.state_dir,
        )
        self.assertTrue(success)
        self.assertFalse(cfg_file.exists())
        self.assertFalse((self.state_dir / "state.json").exists())

    def test_apply_grants_invalid_json(self) -> None:
        """Verify invalid JSON in existing config.json is gracefully handled."""
        cfg_file = self.mock_root / "config.json"
        cfg_file.write_text("{invalid_json", encoding="utf-8")

        success = config.apply_grants(
            config_path=cfg_file,
            allow=["command(fx build)"],
            deny=[],
            ask=[],
            dry_run=False,
            state_dir=self.state_dir,
        )
        self.assertFalse(success)
        self.assertIn("Error reading JSON from", self.stderr)


if __name__ == "__main__":
    unittest.main()
