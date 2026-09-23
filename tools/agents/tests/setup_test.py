#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the agents setup command handler."""

from __future__ import annotations

import argparse
import json
import unittest
from unittest import mock

from agents.commands import setup
from agents.lib import githooks, paths, state
from agents_testing.base import BaseTestCase


class SetupCommandTest(BaseTestCase):
    """Tests for setup command parser and execution."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_root = self.test_dir
        self.state_dir = self.mock_root / "state"
        self.backups_dir = self.state_dir / "backups"
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self.backups_dir.mkdir(parents=True, exist_ok=True)

        self.fuchsia_dir = self.mock_root / "fuchsia"
        self.perm_dir = self.fuchsia_dir / ".agents" / "config" / "permissions"
        self.perm_dir.mkdir(parents=True, exist_ok=True)
        (self.fuchsia_dir / ".git").mkdir(parents=True, exist_ok=True)

        self.patch_object(
            paths, "find_fuchsia_dir", return_value=self.fuchsia_dir
        )

        perm_files = {
            "read_only.txt": "fx status\n",
            "local_changes.txt": "git commit\n",
            "never_allow.txt": "git clean\n",
            "device_ops.txt": "fx ota\n",
            "cache_destruction.txt": "fx clean\n",
            "batch_execution.txt": "find\n",
            "external_changes.txt": "git push\n",
        }
        for name, content in perm_files.items():
            (self.perm_dir / name).write_text(content, encoding="utf-8")

    def test_add_arguments(self) -> None:
        """Verify CLI arguments are added correctly."""
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)

        args = parser.parse_args(
            [
                "--profile",
                "read-only",
                "--allow",
                "fx build",
                "--deny",
                "git push --force",
                "--dry-run",
            ]
        )
        self.assertEqual(args.profile, "read-only")
        self.assertEqual(args.allow, ["fx build"])
        self.assertEqual(args.deny, ["git push --force"])
        self.assertTrue(args.dry_run)

    def test_run_dry_run(self) -> None:
        """Verify dry-run mode does not create or modify files."""
        config_path = self.mock_root / "config.json"
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "--allow",
                "fx build",
                "--dry-run",
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)
        self.assertFalse(config_path.exists())
        self.assertIn("[DRY RUN]", self.stdout)

    def test_run_apply_grants(self) -> None:
        """Verify grants are applied and written to config."""
        config_path = self.mock_root / "config.json"
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "--allow",
                "fx build",
                "--deny",
                "git push --force",
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)
        self.assertTrue(config_path.exists())

        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "fx" in g and "build" in g
                for g in grants["allow"]
            )
        )
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "git" in g and "push" in g
                for g in grants["deny"]
            )
        )

    def test_run_with_profile(self) -> None:
        """Verify setting profile populates category grants."""
        config_path = self.mock_root / "config.json"
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "-p",
                "read-only",
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)

        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "fx" in g and "status" in g
                for g in grants["allow"]
            )
        )
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "git" in g and "clean" in g
                for g in grants["deny"]
            )
        )

    def test_run_default_profile(self) -> None:
        """Verify running setup without explicit args defaults to local-changes profile."""
        config_path = self.mock_root / "config.json"
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            ["--config", str(config_path), "--state-dir", str(self.state_dir)]
        )

        exit_code = setup.run(args)

        self.assertEqual(exit_code, 0)
        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "fx" in g and "status" in g
                for g in grants["allow"]
            )
        )
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "git" in g and "commit" in g
                for g in grants["allow"]
            )
        )

    def test_run_with_list_files(self) -> None:
        """Verify reading extra list files."""
        config_path = self.mock_root / "config.json"
        list_file = self.mock_root / "extra_allow.txt"
        list_file.write_text("my_custom_tool\n", encoding="utf-8")

        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "--allow-list",
                str(list_file),
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)

        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "my_custom_tool" in g
                for g in grants["allow"]
            )
        )

    def test_run_status(self) -> None:
        """Verify --status option prints status summary."""
        config_path = self.mock_root / "config.json"
        config_path.write_text('{"userSettings": {}}', encoding="utf-8")

        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--status",
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)
        self.assertIn(
            "=== AI Coding Agent Configuration Status ===",
            self.stdout,
        )

    def test_run_rollback(self) -> None:
        """Verify --rollback executes rollback."""
        config_path = self.mock_root / "config.json"
        config_path.write_text('{"v": 1}', encoding="utf-8")
        backup = state.create_backup(config_path, self.backups_dir)

        journal = state.StateJournal(
            active_profile="local-changes",
            history=[
                {
                    "timestamp": "t1",
                    "profile": "read-only",
                    "backup_file": None,
                },
                {
                    "timestamp": "t2",
                    "profile": "local-changes",
                    "backup_file": str(backup),
                },
            ],
            managed_grants={"allow": [], "deny": [], "ask": []},
        )
        state.save_state(journal, self.state_dir / "state.json")

        config_path.write_text('{"v": 2}', encoding="utf-8")

        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--rollback",
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)
        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        self.assertEqual(data, {"v": 1})

    def test_run_reset(self) -> None:
        """Verify --reset executes reset."""
        config_path = self.mock_root / "config.json"
        config_path.write_text(
            json.dumps(
                {
                    "userSettings": {
                        "globalPermissionGrants": {
                            "allow": [
                                "command(fx status)",
                                "command(custom_tool)",
                            ],
                            "deny": ["command(git clean)"],
                            "ask": [],
                        }
                    }
                }
            ),
            encoding="utf-8",
        )

        journal = state.StateJournal(
            active_profile="local-changes",
            history=[
                {
                    "timestamp": "t1",
                    "profile": "local-changes",
                    "backup_file": None,
                }
            ],
            managed_grants={
                "allow": ["command(fx status)"],
                "deny": ["command(git clean)"],
                "ask": [],
            },
        )
        state.save_state(journal, self.state_dir / "state.json")

        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--reset",
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
            ]
        )

        exit_code = setup.run(args)
        self.assertEqual(exit_code, 0)

        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertEqual(grants["allow"], ["command(custom_tool)"])
        self.assertEqual(grants["deny"], [])
        self.assertIn("Purged Fuchsia-managed rules", self.stdout)

    def test_run_restarts_daemons_on_success(self) -> None:
        """Verify daemons are restarted on apply, rollback, and reset."""
        config_path = self.mock_root / "config.json"
        config_path.write_text("{}", encoding="utf-8")
        backup = state.create_backup(config_path, self.backups_dir)

        journal = state.StateJournal(
            active_profile="local-changes",
            history=[
                {
                    "timestamp": "t1",
                    "profile": "local-changes",
                    "backup_file": str(backup),
                }
            ],
            managed_grants={"allow": [], "deny": [], "ask": []},
        )
        state.save_state(journal, self.state_dir / "state.json")

        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)

        with mock.patch.object(setup, "_restart_daemons") as mock_restart:
            # Test apply
            args_apply = parser.parse_args(
                [
                    "--config",
                    str(config_path),
                    "--state-dir",
                    str(self.state_dir),
                    "-p",
                    "read-only",
                ]
            )
            setup.run(args_apply)
            mock_restart.assert_called_with(self.fuchsia_dir, False)
            mock_restart.reset_mock()

            # Test rollback
            args_rollback = parser.parse_args(
                [
                    "--rollback",
                    "--config",
                    str(config_path),
                    "--state-dir",
                    str(self.state_dir),
                ]
            )
            setup.run(args_rollback)
            mock_restart.assert_called_with(self.fuchsia_dir, False)
            mock_restart.reset_mock()

            # Test reset
            args_reset = parser.parse_args(
                [
                    "--reset",
                    "--config",
                    str(config_path),
                    "--state-dir",
                    str(self.state_dir),
                ]
            )
            setup.run(args_reset)
            mock_restart.assert_called_with(self.fuchsia_dir, False)

    def test_run_adhoc_grant_preserves_active_profile(self) -> None:
        """Verify adding an ad-hoc grant preserves the existing active profile."""
        config_path = self.mock_root / "config.json"
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)

        # 1. Apply read-only profile first
        args_init = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "-p",
                "read-only",
            ]
        )
        self.assertEqual(setup.run(args_init), 0)

        # Verify active profile is read-only
        j1 = state.load_state(self.state_dir / "state.json")
        self.assertEqual(j1.active_profile, "read-only")

        # 2. Add ad-hoc grant without specifying --profile
        args_adhoc = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "--allow",
                "fx custom_tool",
            ]
        )
        self.assertEqual(setup.run(args_adhoc), 0)

        # Verify active profile remains read-only
        j2 = state.load_state(self.state_dir / "state.json")
        self.assertEqual(j2.active_profile, "read-only")

        # Verify config contains both read-only rules and custom tool rule
        with config_path.open("r", encoding="utf-8") as fh:
            data = json.load(fh)
        grants = data["userSettings"]["globalPermissionGrants"]
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "status" in g
                for g in grants["allow"]
            )
        )
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "custom_tool" in g
                for g in grants["allow"]
            )
        )

    def test_git_hooks_cli_flags(self) -> None:
        """Verify BooleanOptionalAction flags for --git-hooks and --no-git-hooks."""
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)

        args_default = parser.parse_args([])
        self.assertTrue(args_default.git_hooks)

        args_explicit = parser.parse_args(["--git-hooks"])
        self.assertTrue(args_explicit.git_hooks)

        args_negated = parser.parse_args(["--no-git-hooks"])
        self.assertFalse(args_negated.git_hooks)

    def test_mutually_exclusive_modes(self) -> None:
        """Verify --status, --rollback, and --reset cannot be combined."""
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)

        with self.assertRaises(SystemExit):
            parser.parse_args(["--reset", "--status"])

        with self.assertRaises(SystemExit):
            parser.parse_args(["--reset", "--rollback"])

        with self.assertRaises(SystemExit):
            parser.parse_args(["--status", "--rollback"])

    def test_setup_git_hooks_installation(self) -> None:
        """Verify git hooks installation behavior during setup across configurations."""
        config_path = self.mock_root / "config.json"
        failed_repo = self.mock_root / "broken_repo"
        pre_commit_hook = (
            self.fuchsia_dir
            / ".git"
            / "hooks"
            / "pre-commit.d"
            / "10-fuchsia-agent.sh"
        )
        commit_msg_hook = (
            self.fuchsia_dir
            / ".git"
            / "hooks"
            / "commit-msg.d"
            / "10-fuchsia-agent.sh"
        )

        cases = [
            (
                "normal",
                ["-p", "read-only"],
                githooks.HookOperationResult(modified=[pre_commit_hook]),
                False,
                "Configured 1 Git hook across checkout.",
                None,
            ),
            (
                "dry_run",
                ["-p", "read-only", "--dry-run"],
                githooks.HookOperationResult(
                    modified=[pre_commit_hook, commit_msg_hook]
                ),
                True,
                "[DRY RUN] Would configure 2 Git hooks across checkout.",
                None,
            ),
            (
                "zero_hooks",
                ["-p", "read-only"],
                githooks.HookOperationResult(modified=[]),
                False,
                "No Git hooks configured.",
                None,
            ),
            (
                "zero_hooks_dry_run",
                ["-p", "read-only", "--dry-run"],
                githooks.HookOperationResult(modified=[]),
                True,
                "[DRY RUN] No Git hooks configured.",
                None,
            ),
            (
                "failure_warnings",
                ["-p", "read-only"],
                githooks.HookOperationResult(
                    modified=[pre_commit_hook],
                    failed=[(failed_repo, "Permission denied")],
                ),
                False,
                "Configured 1 Git hook across checkout.",
                f"Warning: Failed to configure Git hooks in {failed_repo}: Permission denied",
            ),
        ]

        for (
            name,
            extra_args,
            result,
            expected_dry_run,
            expected_stdout,
            expected_stderr,
        ) in cases:
            with self.subTest(scenario=name):
                self.mock_stdout.truncate(0)
                self.mock_stdout.seek(0)
                self.mock_stderr.truncate(0)
                self.mock_stderr.seek(0)

                parser = argparse.ArgumentParser()
                setup.add_arguments(parser)
                args = parser.parse_args(
                    [
                        "--config",
                        str(config_path),
                        "--state-dir",
                        str(self.state_dir),
                        *extra_args,
                    ]
                )

                with mock.patch(
                    "agents.lib.githooks.install_git_hooks",
                    return_value=result,
                ) as mock_install:
                    self.assertEqual(setup.run(args), 0)
                    mock_install.assert_called_once_with(
                        self.fuchsia_dir, dry_run=expected_dry_run
                    )
                    self.assertIn(expected_stdout, self.stdout)
                    if expected_stderr:
                        self.assertIn(expected_stderr, self.stderr)

    def test_setup_no_git_hooks_flag_skips_install(self) -> None:
        """Verify --no-git-hooks skips hook installation."""
        config_path = self.mock_root / "config.json"
        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
                "-p",
                "read-only",
                "--no-git-hooks",
            ]
        )
        with mock.patch(
            "agents.lib.githooks.install_git_hooks"
        ) as mock_install:
            self.assertEqual(setup.run(args), 0)
            mock_install.assert_not_called()

    def test_setup_reset_git_hooks(self) -> None:
        """Verify git hooks uninstallation during reset across configurations."""
        config_path = self.mock_root / "config.json"
        failed_repo = self.mock_root / "broken_repo"
        pre_commit_hook = (
            self.fuchsia_dir
            / ".git"
            / "hooks"
            / "pre-commit.d"
            / "10-fuchsia-agent.sh"
        )
        commit_msg_hook = (
            self.fuchsia_dir
            / ".git"
            / "hooks"
            / "commit-msg.d"
            / "10-fuchsia-agent.sh"
        )

        cases = [
            (
                "normal",
                ["--reset"],
                githooks.HookOperationResult(
                    modified=[pre_commit_hook, commit_msg_hook]
                ),
                False,
                "Removed 2 Git hooks across checkout.",
                None,
            ),
            (
                "dry_run",
                ["--reset", "--dry-run"],
                githooks.HookOperationResult(modified=[pre_commit_hook]),
                True,
                "[DRY RUN] Would remove 1 Git hook across checkout.",
                None,
            ),
            (
                "zero_hooks",
                ["--reset"],
                githooks.HookOperationResult(modified=[]),
                False,
                "No Git hooks found to remove.",
                None,
            ),
            (
                "zero_hooks_dry_run",
                ["--reset", "--dry-run"],
                githooks.HookOperationResult(modified=[]),
                True,
                "[DRY RUN] No Git hooks found to remove.",
                None,
            ),
            (
                "failure_warnings",
                ["--reset"],
                githooks.HookOperationResult(
                    modified=[],
                    failed=[(failed_repo, "File is locked")],
                ),
                False,
                None,
                f"Warning: Failed to remove Git hooks in {failed_repo}: File is locked",
            ),
        ]

        for (
            name,
            extra_args,
            result,
            expected_dry_run,
            expected_stdout,
            expected_stderr,
        ) in cases:
            with self.subTest(scenario=name):
                self.mock_stdout.truncate(0)
                self.mock_stdout.seek(0)
                self.mock_stderr.truncate(0)
                self.mock_stderr.seek(0)
                config_path.write_text('{"userSettings": {}}', encoding="utf-8")

                parser = argparse.ArgumentParser()
                setup.add_arguments(parser)
                args = parser.parse_args(
                    [
                        "--config",
                        str(config_path),
                        "--state-dir",
                        str(self.state_dir),
                        *extra_args,
                    ]
                )

                with (
                    mock.patch(
                        "agents.lib.githooks.uninstall_git_hooks",
                        return_value=result,
                    ) as mock_uninstall,
                    mock.patch("agents.lib.state.reset", return_value=True),
                ):
                    self.assertEqual(setup.run(args), 0)
                    mock_uninstall.assert_called_once_with(
                        self.fuchsia_dir, dry_run=expected_dry_run
                    )
                    if expected_stdout:
                        self.assertIn(expected_stdout, self.stdout)
                    if expected_stderr:
                        self.assertIn(expected_stderr, self.stderr)

    def test_setup_reset_with_no_git_hooks_skips_uninstall(self) -> None:
        """Verify reset with --no-git-hooks resets state but skips uninstalling hooks."""
        config_path = self.mock_root / "config.json"
        config_path.write_text('{"userSettings": {}}', encoding="utf-8")

        parser = argparse.ArgumentParser()
        setup.add_arguments(parser)
        args = parser.parse_args(
            [
                "--reset",
                "--no-git-hooks",
                "--config",
                str(config_path),
                "--state-dir",
                str(self.state_dir),
            ]
        )

        with (
            mock.patch(
                "agents.lib.githooks.uninstall_git_hooks"
            ) as mock_uninstall,
            mock.patch(
                "agents.lib.state.reset", return_value=True
            ) as mock_reset,
        ):
            self.assertEqual(setup.run(args), 0)
            mock_reset.assert_called_once_with(
                config_path=config_path,
                state_path=self.state_dir / "state.json",
                backups_dir=self.backups_dir,
                dry_run=False,
            )
            mock_uninstall.assert_not_called()

    def test_setup_no_repositories_found(self) -> None:
        """Verify report when no Git repositories are found across checkout."""
        config_path = self.mock_root / "config.json"
        for mode, extra_args, hook_func in [
            (
                "setup",
                ["-p", "read-only"],
                "agents.lib.githooks.install_git_hooks",
            ),
            ("reset", ["--reset"], "agents.lib.githooks.uninstall_git_hooks"),
        ]:
            with self.subTest(mode=mode):
                self.mock_stdout.truncate(0)
                self.mock_stdout.seek(0)
                config_path.write_text('{"userSettings": {}}', encoding="utf-8")
                parser = argparse.ArgumentParser()
                setup.add_arguments(parser)
                args = parser.parse_args(
                    [
                        "--config",
                        str(config_path),
                        "--state-dir",
                        str(self.state_dir),
                        *extra_args,
                    ]
                )
                with (
                    mock.patch(
                        "agents.lib.paths.find_checkout_git_repos",
                        return_value=[],
                    ),
                    mock.patch(
                        hook_func,
                        return_value=githooks.HookOperationResult(modified=[]),
                    ),
                    mock.patch("agents.lib.state.reset", return_value=True),
                ):
                    self.assertEqual(setup.run(args), 0)
                    self.assertIn(
                        "No Git repositories found across checkout.",
                        self.stdout,
                    )

    def test_setup_status_reports_git_hooks(self) -> None:
        """Verify status reports Git hooks configuration status across repositories."""
        cases = [
            (1, 1, "Git hooks: Configured across 1/1 repository."),
            (2, 2, "Git hooks: Configured across 2/2 repositories."),
        ]
        config_path = self.mock_root / "config.json"
        config_path.write_text('{"userSettings": {}}', encoding="utf-8")

        for total, configured, expected_str in cases:
            with self.subTest(total=total, configured=configured):
                self.mock_stdout.truncate(0)
                self.mock_stdout.seek(0)

                parser = argparse.ArgumentParser()
                setup.add_arguments(parser)
                args = parser.parse_args(
                    [
                        "--status",
                        "--config",
                        str(config_path),
                        "--state-dir",
                        str(self.state_dir),
                    ]
                )

                with mock.patch(
                    "agents.lib.githooks.get_git_hooks_status"
                ) as mock_status:
                    mock_status.return_value = githooks.HookStatusResult(
                        total_repos=total, configured_repos=configured
                    )
                    self.assertEqual(setup.run(args), 0)
                    mock_status.assert_called_once_with(self.fuchsia_dir)
                    self.assertIn(expected_str, self.stdout)


if __name__ == "__main__":
    unittest.main()
