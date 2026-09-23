#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import os
import subprocess
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

from agents.lib.githooks import adapters
from agents.lib.githooks.reporters import ConsoleReporter
from agents_testing.base import BaseTestCase


class AdaptersTest(unittest.TestCase):
    """Unit tests for githooks.adapters dataclasses and definitions."""

    def test_hook_action_definition(self) -> None:
        action = adapters.HookAction(
            name="test_action",
            action_fn=lambda ctx, files: True,
            is_mutating=True,
            extensions=[".py"],
            remediation_cmd_fn=lambda files: [f"fix {','.join(files)}"],
        )
        self.assertEqual(action.name, "test_action")
        self.assertTrue(action.is_mutating)
        self.assertEqual(action.extensions, [".py"])
        self.assertIsNotNone(action.remediation_cmd_fn)
        if action.remediation_cmd_fn:
            self.assertEqual(action.remediation_cmd_fn(["a.py"]), ["fix a.py"])
        self.assertTrue(len(adapters.DEFAULT_PRE_COMMIT_ACTIONS) > 0)
        default_cmd_fn = adapters.DEFAULT_PRE_COMMIT_ACTIONS[
            0
        ].remediation_cmd_fn
        self.assertIsNotNone(default_cmd_fn)
        if default_cmd_fn:
            self.assertEqual(
                default_cmd_fn(["a.py"]),
                ["fx format-code --files=a.py"],
            )


class FormatCodeActionTest(BaseTestCase):
    """Unit tests for format_code_action using HookContext.reporter."""

    def make_mock_reporter(
        self,
        spec: Any = None,
    ) -> mock.MagicMock:
        """Factory helper to construct a mock ConsoleReporter."""
        return mock.MagicMock(spec=spec) if spec else mock.MagicMock()

    def setUp(self) -> None:
        super().setUp()
        (self.test_dir / ".fx-root").touch()

    def test_format_code_action_empty(self) -> None:
        ctx = adapters.HookContext(repo_root=self.test_dir)
        self.assertTrue(adapters.format_code_action(ctx, []))

    def test_format_code_action_runs_fx_format_code(self) -> None:
        ctx = adapters.HookContext(repo_root=self.test_dir)
        with (
            mock.patch("shutil.which", return_value="/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = self.make_completed_process()
            ok = adapters.format_code_action(ctx, ["a.py", "b.cc"])
            self.assertTrue(ok)
            mock_cmd.assert_called_once_with(
                ["fx", "format-code", "--files=a.py,b.cc"],
                cwd=self.test_dir,
            )

    def test_format_code_action_error_reporting(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        with (
            mock.patch("shutil.which", return_value="/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = self.make_completed_process(
                returncode=1, stderr="Error formatting code"
            )
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once_with(
                "Error formatting code"
            )

    def test_format_code_action_check_only_modified_non_destructive_returns_false(
        self,
    ) -> None:
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            check_only=True,
        )
        with (
            mock.patch("shutil.which", return_value="/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.side_effect = [
                # 1. fx format-code
                self.make_completed_process(),
                # 2. git diff --quiet -- foo.py
                self.make_completed_process(returncode=1),
            ]
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertFalse(ok)
            self.assertEqual(
                mock_cmd.call_args_list,
                [
                    mock.call(
                        ["fx", "format-code", "--files=foo.py"],
                        cwd=self.test_dir,
                    ),
                    mock.call(
                        ["git", "diff", "--quiet", "--", "foo.py"],
                        cwd=self.test_dir,
                    ),
                ],
            )

    def test_format_code_action_check_only_unmodified_returns_true(
        self,
    ) -> None:
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            check_only=True,
        )
        with (
            mock.patch("shutil.which", return_value="/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.side_effect = [
                # 1. fx format-code
                self.make_completed_process(),
                # 2. git diff --quiet -- foo.py
                self.make_completed_process(returncode=0),
            ]
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertTrue(ok)
            self.assertEqual(
                mock_cmd.call_args_list,
                [
                    mock.call(
                        ["fx", "format-code", "--files=foo.py"],
                        cwd=self.test_dir,
                    ),
                    mock.call(
                        ["git", "diff", "--quiet", "--", "foo.py"],
                        cwd=self.test_dir,
                    ),
                ],
            )

    def test_format_code_action_resolves_fx_from_scripts_when_not_in_path(
        self,
    ) -> None:
        scripts_dir = self.test_dir / "scripts"
        scripts_dir.mkdir(parents=True, exist_ok=True)
        fx_script = scripts_dir / "fx"
        fx_script.touch()

        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
        )
        with (
            mock.patch("shutil.which", return_value=None),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = self.make_completed_process()
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertTrue(ok)
            mock_cmd.assert_called_once_with(
                [str(fx_script), "format-code", "--files=foo.py"],
                cwd=self.test_dir,
            )

    def test_format_code_action_prioritizes_scripts_fx_over_ambient_path(
        self,
    ) -> None:
        scripts_dir = self.test_dir / "scripts"
        scripts_dir.mkdir(parents=True, exist_ok=True)
        fx_script = scripts_dir / "fx"
        fx_script.touch()

        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
        )
        with (
            mock.patch("shutil.which", return_value="/ambient/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = self.make_completed_process()
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertTrue(ok)
            mock_cmd.assert_called_once_with(
                [str(fx_script), "format-code", "--files=foo.py"],
                cwd=self.test_dir,
            )

    def test_format_code_action_missing_fx_not_in_path_or_scripts_human(
        self,
    ) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        with (
            mock.patch("shutil.which", return_value=None),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertTrue(ok)
            mock_reporter.on_warning.assert_called_once()
            mock_cmd.assert_not_called()

    def test_format_code_action_missing_fx_not_in_path_or_scripts_agent(
        self,
    ) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with (
            mock.patch("shutil.which", return_value=None),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once()
            mock_cmd.assert_not_called()

    def test_format_code_action_combined_stdout_and_stderr_error(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with (
            mock.patch("shutil.which", return_value="/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=1,
                stdout="Formatter stdout output",
                stderr="Formatter stderr output",
            )
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once_with(
                "Formatter stdout output\nFormatter stderr output"
            )

    def test_format_code_action_empty_output_fallback_error(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with (
            mock.patch("shutil.which", return_value="/bin/fx"),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = self.make_completed_process(
                returncode=2, stderr=""
            )
            ok = adapters.format_code_action(ctx, ["foo.py"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once()
            err_msg = mock_reporter.on_error.call_args[0][0]
            self.assertIn("failed with exit code 2", err_msg)


class CommitMsgActionTest(BaseTestCase):
    """Unit tests for commit_msg_action using HookContext.reporter."""

    def make_mock_reporter(
        self,
        spec: Any = None,
    ) -> mock.MagicMock:
        """Factory helper to construct a mock ConsoleReporter."""
        return mock.MagicMock(spec=spec) if spec else mock.MagicMock()

    def setUp(self) -> None:
        super().setUp()
        (self.test_dir / ".fx-root").touch()
        self.checker_script = (
            self.test_dir / "scripts" / "shac" / "commit_msg_checker.py"
        )
        self.checker_script.parent.mkdir(parents=True, exist_ok=True)
        self.checker_script.touch()

    def test_commit_msg_action_empty(self) -> None:
        ctx = adapters.HookContext(repo_root=self.test_dir)
        self.assertTrue(adapters.commit_msg_action(ctx, []))

    def test_commit_msg_action_runs_checker_strict_for_agent(self) -> None:
        ctx = adapters.HookContext(repo_root=self.test_dir, is_agent=True)
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = self.make_completed_process()
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertTrue(ok)
            mock_cmd.assert_called_once()
            cmd_args = mock_cmd.call_args[0][0]
            self.assertIn("--strict", cmd_args)

    def test_commit_msg_action_error_reporting(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = self.make_completed_process(
                returncode=1, stderr="Title too long"
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once_with("Title too long")

    def test_commit_msg_action_warning_human(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=0,
                stdout="Warning: check format",
                stderr="",
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertTrue(ok)
            mock_reporter.on_warning.assert_called_once_with(
                "Warning: check format"
            )

    def test_commit_msg_action_warning_human_with_stderr_noise(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=0,
                stdout="Warning: subject line exceeds 50 chars",
                stderr="Some python warning\nDeprecationWarning: ...\n",
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertTrue(ok)
            mock_reporter.on_warning.assert_called_once_with(
                "Warning: subject line exceeds 50 chars"
            )

    def test_commit_msg_action_success_with_stderr_noise_no_warning(
        self,
    ) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = self.make_completed_process(
                returncode=0,
                stderr="Some python warning\n",
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertTrue(ok)
            mock_reporter.on_warning.assert_not_called()

    def test_commit_msg_action_error_combines_stdout_and_stderr(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=1,
                stdout="[ERROR] Missing Change-Id line",
                stderr="Traceback (most recent call last):\n  File 'test.py', line 1\n",
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once_with(
                "[ERROR] Missing Change-Id line\nTraceback (most recent call last):\n  File 'test.py', line 1"
            )

    def test_commit_msg_action_empty_output_fallback_error(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = self.make_completed_process(
                returncode=3, stderr=""
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once()
            err_msg = mock_reporter.on_error.call_args[0][0]
            self.assertIn("failed with exit code 3", err_msg)

    def test_commit_msg_action_checker_script_missing_human(self) -> None:
        self.checker_script.unlink()
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
        self.assertTrue(ok)
        mock_reporter.on_warning.assert_called_once()
        mock_reporter.on_error.assert_not_called()

    def test_commit_msg_action_checker_script_missing_agent(self) -> None:
        self.checker_script.unlink()
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
        self.assertFalse(ok)
        mock_reporter.on_error.assert_called_once()
        mock_reporter.on_warning.assert_not_called()

    def test_commit_msg_action_multiline_warnings_human(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=False,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=0,
                stdout="Warning: line 1\n\n  Warning: line 2  \n",
                stderr="",
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertTrue(ok)
            mock_reporter.on_warning.assert_called_once_with(
                "Warning: line 1\n\n  Warning: line 2"
            )

    def test_commit_msg_action_multiline_errors_agent(self) -> None:
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=1,
                stdout="Error: line 1\n\n  Error: line 2  \n",
                stderr="",
            )
            ok = adapters.commit_msg_action(ctx, ["COMMIT_EDITMSG"])
            self.assertFalse(ok)
            mock_reporter.on_error.assert_called_once_with(
                "Error: line 1\n\n  Error: line 2"
            )

    def test_commit_msg_action_relative_path_from_subdirectory(self) -> None:
        subdir = self.test_dir / "nested" / "sub"
        subdir.mkdir(parents=True, exist_ok=True)
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        ctx = adapters.HookContext(
            repo_root=self.test_dir,
            is_agent=True,
            reporter=mock_reporter,
        )
        with mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd:
            mock_cmd.return_value = self.make_completed_process()
            self.addCleanup(os.chdir, Path.cwd())
            os.chdir(subdir)
            ok = adapters.commit_msg_action(ctx, [".git/COMMIT_EDITMSG"])
            self.assertTrue(ok)
            mock_cmd.assert_called_once()
            cmd_args = mock_cmd.call_args[0][0]
            self.assertIn(".git/COMMIT_EDITMSG", cmd_args)


if __name__ == "__main__":
    unittest.main()
