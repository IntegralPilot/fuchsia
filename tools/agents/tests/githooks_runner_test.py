#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import dataclasses
import io
import os
import shutil
import subprocess
import tempfile
import unittest
from collections.abc import Sequence
from pathlib import Path
from typing import Any
from unittest import mock

from agents.lib import git_staging, githooks
from agents.lib.githooks import adapters, runner
from agents.lib.githooks.reporters import ConsoleReporter
from agents_testing.workspace import GitWorkspaceTestCase


class AgentDetectionTest(unittest.TestCase):
    """Unit tests for agent detection."""

    def test_is_invoked_by_agent(self) -> None:
        self.assertFalse(runner.is_invoked_by_agent(env={}))
        self.assertFalse(runner.is_invoked_by_agent(env={"USER": "alice"}))

        for var in (
            "ANTIGRAVITY_AGENT",
            "GEMINI_CLI",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
        ):
            self.assertTrue(runner.is_invoked_by_agent(env={var: "1"}))

        for val in ("1", "true", "yes", "y", "on"):
            self.assertTrue(
                runner.is_invoked_by_agent(env={"ANTIGRAVITY_AGENT": val})
            )

        for val in ("0", "false", "no", "n", "off", ""):
            self.assertFalse(
                runner.is_invoked_by_agent(env={"ANTIGRAVITY_AGENT": val})
            )


class RunnerTest(GitWorkspaceTestCase):
    """Unit tests for githooks runner."""

    def make_mock_reporter(
        self,
        spec: Any = None,
        has_errors: bool = False,
    ) -> mock.MagicMock:
        """Factory helper to construct a mock ConsoleReporter."""
        reporter = mock.MagicMock(spec=spec) if spec else mock.MagicMock()
        reporter.has_errors = has_errors
        return reporter

    def test_run_pre_commit_hook_skip_env(self) -> None:
        for val in ("1", "true", "yes", "on"):
            with mock.patch.dict(os.environ, {"FUCHSIA_SKIP_HOOKS": val}):
                self.assertEqual(githooks.run_pre_commit_hook(), 0)

    def test_run_pre_commit_hook_not_in_git_repo(self) -> None:
        with tempfile.TemporaryDirectory() as non_git:
            reporter = ConsoleReporter(
                stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
            )
            ret = githooks.run_pre_commit_hook(
                repo_dir=Path(non_git), reporter=reporter
            )
            self.assertEqual(ret, 1)

    def test_run_pre_commit_hook_success(self) -> None:
        mock_fmt = mock.MagicMock(return_value=True)
        action = adapters.HookAction(
            name="mock_fmt", action_fn=mock_fmt, is_mutating=True
        )
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_pre_commit_hook(
            repo_dir=self.test_dir,
            actions=[action],
            reporter=reporter,
        )
        self.assertEqual(ret, 0)

    def test_run_pre_commit_hook_restaged_files(self) -> None:
        pipeline_result = git_staging.PipelineResult(
            success=True,
            restaged_files=("foo.py",),
        )
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            return_value=pipeline_result,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 0)
            mock_reporter.on_files_restaged.assert_called_once_with(
                ["foo.py"],
                message="Auto-formatted and re-staged",
            )
            mock_reporter.finish.assert_called_once()

    def test_run_pre_commit_hook_partial_staging_conflict(self) -> None:
        pipeline_result = git_staging.PipelineResult(
            success=False,
            partial_conflicts=("bar.py",),
            error="Custom error",
        )
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            return_value=pipeline_result,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 1)
            mock_reporter.on_partial_staging_conflict.assert_called_once_with(
                ["bar.py"],
                message="Custom error",
                remediation_cmds=["fx format-code --files=bar.py"],
            )
            mock_reporter.finish.assert_called_once()

    def test_run_pre_commit_hook_partial_staging_conflict_default_message(
        self,
    ) -> None:
        pipeline_result = git_staging.PipelineResult(
            success=False,
            partial_conflicts=("bar.py",),
            error="",
        )
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            return_value=pipeline_result,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 1)
            mock_reporter.on_partial_staging_conflict.assert_called_once_with(
                ["bar.py"],
                message=(
                    "Formatting issues in partially staged files. Auto-fix"
                    " skipped to prevent stash conflicts."
                ),
                remediation_cmds=["fx format-code --files=bar.py"],
            )

    def test_run_pre_commit_hook_partial_staging_conflict_custom_remediation(
        self,
    ) -> None:
        pipeline_result = git_staging.PipelineResult(
            success=False,
            partial_conflicts=("bar.py", "baz.py"),
            error="Custom error",
        )
        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        custom_action = adapters.HookAction(
            name="custom_action",
            action_fn=lambda ctx, files: True,
            is_mutating=True,
            remediation_cmd_fn=lambda files: [f"custom-fix {' '.join(files)}"],
        )
        action_without_remediation = adapters.HookAction(
            name="no_remediation",
            action_fn=lambda ctx, files: True,
            is_mutating=False,
        )
        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            return_value=pipeline_result,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                actions=[custom_action, action_without_remediation],
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 1)
            mock_reporter.on_partial_staging_conflict.assert_called_once_with(
                ["bar.py", "baz.py"],
                message="Custom error",
                remediation_cmds=["custom-fix bar.py baz.py"],
            )

    def test_run_pre_commit_hook_multiple_actions_filter_by_extensions(
        self,
    ) -> None:
        action1_files: list[list[str]] = []
        action2_files: list[list[str]] = []

        def record_action1(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            action1_files.append(list(files))
            return True

        def record_action2(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            action2_files.append(list(files))
            return True

        action1 = adapters.HookAction(
            name="py_action",
            action_fn=record_action1,
            is_mutating=True,
            extensions=(".py",),
            remediation_cmd_fn=lambda files: [f"py-fix {' '.join(files)}"],
        )
        action2 = adapters.HookAction(
            name="cc_action",
            action_fn=record_action2,
            is_mutating=True,
            extensions=(".cc",),
            remediation_cmd_fn=lambda files: [f"cc-fix {' '.join(files)}"],
        )

        pipeline_result = git_staging.PipelineResult(
            success=False,
            partial_conflicts=("foo.py", "bar.cc", "baz.txt"),
            error="Partial conflict",
        )

        def invoke_pipeline(
            context: git_staging.ActionContext,
            mutating_actions: Sequence[git_staging.StagedFileAction],
            read_only_checks: Sequence[git_staging.StagedFileAction],
            extensions: Sequence[str] | None = None,
        ) -> git_staging.PipelineResult:
            for act in mutating_actions:
                act(context, ["foo.py", "bar.cc", "baz.txt"])
            return pipeline_result

        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            side_effect=invoke_pipeline,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                actions=[action1, action2],
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 1)
            # Action 1 only receives .py files and Action 2 only receives .cc files
            self.assertEqual(action1_files, [["foo.py"]])
            self.assertEqual(action2_files, [["bar.cc"]])
            # Remediation commands are filtered by action extensions for partial conflicts
            mock_reporter.on_partial_staging_conflict.assert_called_once_with(
                ["foo.py", "bar.cc", "baz.txt"],
                message="Partial conflict",
                remediation_cmds=["py-fix foo.py", "cc-fix bar.cc"],
            )

    def test_run_pre_commit_hook_wrap_action_silenced_in_check_only(
        self,
    ) -> None:
        mock_reporter = self.make_mock_reporter(
            ConsoleReporter, has_errors=False
        )
        action = adapters.HookAction(
            name="failing_action",
            action_fn=lambda ctx, files: False,
            is_mutating=True,
        )

        def invoke_pipeline(
            context: git_staging.ActionContext,
            mutating_actions: Sequence[git_staging.StagedFileAction],
            read_only_checks: Sequence[git_staging.StagedFileAction],
            extensions: Sequence[str] | None = None,
        ) -> git_staging.PipelineResult:
            # Simulate pipeline running mutating action in check_only mode
            check_ctx = dataclasses.replace(context, check_only=True)
            for act in mutating_actions:
                act(check_ctx, ["foo.py"])
            return git_staging.PipelineResult(success=True)

        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            side_effect=invoke_pipeline,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                actions=[action],
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 0)
            mock_reporter.on_error.assert_not_called()

    def test_run_pre_commit_hook_console_reporter_unicode_error_marker(
        self,
    ) -> None:
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(
            stderr_stream=stderr_buf,
            unicode=True,
        )
        pipeline_result = git_staging.PipelineResult(
            success=False,
            partial_conflicts=("bar.py",),
            error="Formatting error in partially staged files",
        )
        with mock.patch(
            "agents.lib.git_staging.run_staged_pipeline",
            return_value=pipeline_result,
        ):
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                reporter=reporter,
            )
            self.assertEqual(ret, 1)
            output = stderr_buf.getvalue()
            self.assertIn(
                "❌ Formatting error in partially staged files", output
            )
            self.assertNotIn("Error:", output)
            self.assertIn("fx format-code --files=bar.py", output)

    def test_run_pre_commit_hook_default_reporter(self) -> None:
        with mock.patch(
            "agents.lib.githooks.runner.ConsoleReporter"
        ) as mock_create:
            mock_create.return_value = self.make_mock_reporter(ConsoleReporter)
            with mock.patch(
                "agents.lib.git_staging.run_staged_pipeline",
                return_value=git_staging.PipelineResult(success=True),
            ):
                with mock.patch.dict(os.environ, {}, clear=True):
                    ret = githooks.run_pre_commit_hook(repo_dir=self.test_dir)
                    self.assertEqual(ret, 0)
                    mock_create.assert_called_once_with()

    def test_run_pre_commit_hook_forwards_reporter_to_format_code_action(
        self,
    ) -> None:
        staged_file = self.stage_file("foo.py", "a = 1\n")

        mock_reporter = self.make_mock_reporter(ConsoleReporter)
        orig_which = shutil.which
        with (
            mock.patch(
                "shutil.which",
                side_effect=lambda cmd: "/bin/fx"
                if cmd == "fx"
                else orig_which(cmd),
            ),
            mock.patch("agents.lib.githooks.adapters.run_cmd") as mock_cmd,
        ):
            mock_cmd.return_value = subprocess.CompletedProcess(
                args=[],
                returncode=1,
                stdout="",
                stderr="Formatting syntax error",
            )
            ret = githooks.run_pre_commit_hook(
                repo_dir=self.test_dir,
                reporter=mock_reporter,
            )
            self.assertEqual(ret, 1)
            mock_reporter.on_error.assert_any_call("Formatting syntax error")
            mock_reporter.finish.assert_called_once()

    def test_run_pre_commit_hook_action_error_suppresses_duplicate_error(
        self,
    ) -> None:
        staged_file = self.stage_file("foo.py", "a = 1\n")

        def failing_action(
            ctx: adapters.HookContext, files: Sequence[str]
        ) -> bool:
            reporter = ctx.reporter
            if reporter is not None:
                reporter.on_error("Action syntax error")
            return False

        action = adapters.HookAction(
            name="failing_action",
            action_fn=failing_action,
            is_mutating=True,
        )
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_pre_commit_hook(
            repo_dir=self.test_dir,
            actions=[action],
            reporter=reporter,
        )
        self.assertEqual(ret, 1)
        self.assertEqual(reporter.errors, ["Action syntax error"])

    def test_run_pre_commit_hook_read_only_action(self) -> None:
        initial_content = "a = 1\n"
        staged_file = self.stage_file("foo.py", initial_content)

        check_calls: list[tuple[bool, list[str]]] = []

        def read_only_check(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            check_calls.append((ctx.check_only, list(files)))
            return True

        action = adapters.HookAction(
            name="lint_check",
            action_fn=read_only_check,
            is_mutating=False,
        )
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_pre_commit_hook(
            repo_dir=self.test_dir,
            actions=[action],
            reporter=reporter,
        )
        self.assertEqual(ret, 0)
        self.assertEqual(check_calls, [(True, ["foo.py"])])
        self.assertEqual(
            staged_file.read_text(encoding="utf-8"), initial_content
        )
        status = self.git("status", "--porcelain")
        self.assertEqual(status.stdout.strip(), "A  foo.py")

    def test_run_pre_commit_hook_action_failure_attributes_action_name(
        self,
    ) -> None:
        staged_file = self.stage_file("foo.py", "a = 1\n")

        action = adapters.HookAction(
            name="custom_check",
            action_fn=lambda ctx, files: False,
            is_mutating=False,
        )
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_pre_commit_hook(
            repo_dir=self.test_dir,
            actions=[action],
            reporter=reporter,
        )
        self.assertEqual(ret, 1)
        self.assertEqual(reporter.errors, ["Action 'custom_check' failed."])

    def test_run_commit_msg_hook_default_reporter(self) -> None:
        msg_file = self.test_dir / "COMMIT_EDITMSG"
        msg_file.write_text("[agents] Test commit\n", encoding="utf-8")
        with mock.patch(
            "agents.lib.githooks.runner.ConsoleReporter"
        ) as mock_create:
            mock_create.return_value = mock.MagicMock(
                spec=ConsoleReporter, has_errors=False
            )
            with mock.patch.dict(os.environ, {}, clear=True):
                ret = githooks.run_commit_msg_hook(
                    str(msg_file),
                    repo_dir=self.test_dir,
                    checker_fn=lambda ctx, files: True,
                )
                self.assertEqual(ret, 0)
                mock_create.assert_called_once_with()

    def test_run_commit_msg_hook_missing_file(self) -> None:
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_commit_msg_hook(
            "non_existent_msg_file",
            repo_dir=self.test_dir,
            reporter=reporter,
        )
        self.assertEqual(ret, 1)

    def test_run_commit_msg_hook_success(self) -> None:
        msg_file = self.test_dir / "COMMIT_EDITMSG"
        msg_file.write_text("[agents] Test commit\n", encoding="utf-8")

        mock_checker = mock.MagicMock(return_value=True)
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_commit_msg_hook(
            str(msg_file),
            repo_dir=self.test_dir,
            checker_fn=mock_checker,
            reporter=reporter,
        )
        self.assertEqual(ret, 0)
        mock_checker.assert_called_once()

    def test_run_commit_msg_hook_worktree(self) -> None:
        worktree_dir = self.create_worktree(
            Path(self.temp_dir.name) / "wt", branch="wt-branch"
        )

        git_dir = worktree_dir / ".git"
        self.assertTrue(git_dir.is_file())  # .git is a file in a worktree

        from agents.lib import paths

        actual_git_dir = paths.get_git_dir(worktree_dir)
        msg_file = actual_git_dir / "COMMIT_EDITMSG"
        msg_file.write_text(
            "[agents] Worktree branch commit\n", encoding="utf-8"
        )

        mock_checker = mock.MagicMock(return_value=True)
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_commit_msg_hook(
            str(msg_file),
            repo_dir=worktree_dir,
            checker_fn=mock_checker,
            reporter=reporter,
        )
        self.assertEqual(ret, 0)
        mock_checker.assert_called_once()
        self.assertEqual(
            str(mock_checker.call_args[0][1][0]), str(msg_file.resolve())
        )

    def test_run_commit_msg_hook_from_subdirectory(self) -> None:
        subdir = self.test_dir / "sub" / "dir"
        subdir.mkdir(parents=True, exist_ok=True)
        msg_file = self.test_dir / ".git" / "COMMIT_EDITMSG"
        msg_file.write_text(
            "[agents] Test commit from subdir\n", encoding="utf-8"
        )

        mock_checker = mock.MagicMock(return_value=True)
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )

        # Relative msg_file_path with repo_dir pointing to subdirectory
        ret = githooks.run_commit_msg_hook(
            ".git/COMMIT_EDITMSG",
            repo_dir=subdir,
            checker_fn=mock_checker,
            reporter=reporter,
        )
        self.assertEqual(ret, 0)
        mock_checker.assert_called_once()
        self.assertEqual(
            str(mock_checker.call_args[0][1][0]), str(msg_file.resolve())
        )

        # Relative msg_file_path when cwd is subdirectory and repo_dir is None
        mock_checker.reset_mock()
        self.addCleanup(os.chdir, Path.cwd())
        os.chdir(subdir)
        ret = githooks.run_commit_msg_hook(
            ".git/COMMIT_EDITMSG",
            checker_fn=mock_checker,
            reporter=reporter,
        )
        self.assertEqual(ret, 0)
        mock_checker.assert_called_once()
        self.assertEqual(
            str(mock_checker.call_args[0][1][0]), str(msg_file.resolve())
        )

    def test_main_routing(self) -> None:
        with mock.patch(
            "agents.lib.githooks.runner.run_commit_msg_hook"
        ) as mock_commit:
            mock_commit.return_value = 0
            ret = githooks.main(["commit-msg", "COMMIT_EDITMSG"])
            self.assertEqual(ret, 0)
            mock_commit.assert_called_once()
            self.assertEqual(
                mock_commit.call_args.kwargs["msg_file_path"], "COMMIT_EDITMSG"
            )

        with mock.patch(
            "agents.lib.githooks.runner.run_pre_commit_hook"
        ) as mock_pre:
            mock_pre.return_value = 0
            ret = githooks.main(["pre-commit"])
            self.assertEqual(ret, 0)
            mock_pre.assert_called_once()

        # Bypass env var
        with mock.patch.dict(os.environ, {"FUCHSIA_SKIP_HOOKS": "1"}):
            self.assertEqual(githooks.main(["pre-commit"]), 0)

        # Invalid subcommand returns 1 or 2 per standard argparse
        with mock.patch("sys.stderr", new_callable=io.StringIO):
            self.assertIn(githooks.main(["unknown-cmd"]), (1, 2))
            self.assertIn(githooks.main([]), (1, 2))

    def test_run_commit_msg_hook_passes_is_agent_to_context(self) -> None:
        msg_file = self.test_dir / "COMMIT_EDITMSG"
        msg_file.write_text("[agents] Test commit\n", encoding="utf-8")

        mock_checker = mock.MagicMock(return_value=True)
        reporter = ConsoleReporter(
            stdout_stream=io.StringIO(), stderr_stream=io.StringIO()
        )
        ret = githooks.run_commit_msg_hook(
            str(msg_file),
            repo_dir=self.test_dir,
            checker_fn=mock_checker,
            reporter=reporter,
            is_agent=True,
        )
        self.assertEqual(ret, 0)
        context = mock_checker.call_args[0][0]
        self.assertTrue(context.is_agent)


if __name__ == "__main__":
    unittest.main()
