#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the git staging isolation and staged action runner library."""

from __future__ import annotations

import re
import subprocess
import tempfile
import unittest
from collections.abc import Sequence
from pathlib import Path
from typing import Any
from unittest import mock

from agents.lib import git_staging
from agents.lib.git_staging import engine
from agents_testing.workspace import GitWorkspaceTestCase


def fake_formatter(
    context: git_staging.ActionContext,
    files: Sequence[str],
) -> bool:
    """Fast in-memory mock formatter for staging logic tests."""
    failed = False
    for rel_path in files:
        file_path = context.repo_root / rel_path
        if not file_path.is_file():
            continue
        content = file_path.read_text(encoding="utf-8")
        formatted = re.sub(
            r"^(\w+)=(\d+)$", r"\1 = \2", content, flags=re.MULTILINE
        )
        if formatted != content:
            if context.check_only:
                failed = True
            else:
                file_path.write_text(formatted, encoding="utf-8")
    return not failed


class GitHelpersTest(GitWorkspaceTestCase):
    """Unit tests for Git helper functions."""

    def test_get_stash_sha(self) -> None:
        self.assertIsNone(engine.get_stash_sha(self.test_dir))


class StagedPartitionTest(GitWorkspaceTestCase):
    """Unit tests for partition_staged_files."""

    def test_partition_fully_and_partially_staged(self) -> None:
        self.stage_file("full.py", "a = 1\n")
        self.stage_partial("partial.py", "b = 1\n", "b = 2\n")
        self.write_file("untracked.py", "c = 1\n")

        partition = git_staging.partition_staged_files(self.test_dir)
        self.assertEqual(partition.fully_staged, frozenset(["full.py"]))
        self.assertEqual(partition.partially_staged, frozenset(["partial.py"]))

    def test_partition_with_extension_filter(self) -> None:
        self.stage_file("full.py", "a = 1\n")
        self.stage_file("full.txt", "txt\n")

        partition = git_staging.partition_staged_files(
            self.test_dir, extensions=[".py"]
        )
        self.assertEqual(partition.fully_staged, frozenset(["full.py"]))


class StashGuardTest(GitWorkspaceTestCase):
    """Unit tests for StashGuard context manager."""

    def test_stash_guard_no_unstaged_changes(self) -> None:
        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertFalse(guard.stashed)
            self.assertIsNone(guard.stash_sha)

    def test_stash_guard_isolates_and_restores_unstaged_modifications(
        self,
    ) -> None:
        f = self.stage_partial("file.txt", "staged\n", "staged and unstaged\n")
        untracked = self.write_file("untracked.txt", "untracked content\n")

        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertTrue(guard.stashed)
            self.assertIsNotNone(guard.stash_sha)
            # Under guard, file content should be only the staged version
            self.assertEqual(f.read_text(encoding="utf-8"), "staged\n")
            # Untracked files remain intact
            self.assertTrue(untracked.is_file())

        # Exiting guard restores unstaged modifications
        self.assertEqual(f.read_text(encoding="utf-8"), "staged and unstaged\n")
        self.assertStashEmpty()
        self.assertFalse(guard.stashed)
        self.assertIsNone(guard.stash_sha)

    def test_stash_guard_isolates_and_restores_crlf(self) -> None:
        f = self.test_dir / "crlf.txt"
        f.write_bytes(b"line1\r\nline2\r\n")
        self.git("add", "crlf.txt")
        f.write_bytes(b"line1\r\nline2\r\nline3\r\n")

        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertTrue(guard.stashed)
            self.assertEqual(f.read_bytes(), b"line1\r\nline2\r\n")

        self.assertEqual(f.read_bytes(), b"line1\r\nline2\r\nline3\r\n")

    def test_stash_guard_isolates_and_restores_raw_binary(self) -> None:
        f = self.test_dir / "binary.bin"
        original_data = b"\xff\x00\x80binary\x00\xff"
        f.write_bytes(original_data)
        self.git("add", "binary.bin")

        modified_data = b"\xff\x00\x80binary\x00\xff\x01\x02"
        f.write_bytes(modified_data)

        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertTrue(guard.stashed)
            self.assertEqual(f.read_bytes(), original_data)

        self.assertEqual(f.read_bytes(), modified_data)

    def test_stash_guard_apply_failure_error_message(self) -> None:
        self.stage_partial("file.txt", "staged\n", "staged and unstaged\n")

        original_run_cmd = engine.run_cmd

        def mock_run_cmd(cmd: Sequence[str], *args: Any, **kwargs: Any) -> Any:
            if cmd[:2] == ["git", "apply"]:
                return subprocess.CompletedProcess(
                    cmd, 1, stdout=b"", stderr=b"error: patch failed\n"
                )
            return original_run_cmd(cmd, *args, **kwargs)

        with mock.patch.object(engine, "run_cmd", side_effect=mock_run_cmd):
            with self.assertRaises(git_staging.GitError) as ctx:
                with git_staging.StashGuard(self.test_dir):
                    pass
            self.assertIn("stash@{0}", str(ctx.exception))
            self.assertIn("git stash apply stash@{0}", str(ctx.exception))
            self.assertIn("error: patch failed", str(ctx.exception))

    def test_stash_guard_preserves_unstaged_deletions(self) -> None:
        """Verifies unstaged deletions of new and modified files are preserved upon exit."""
        new_file = self.stage_deletion("new_file.txt", "staged content\n")
        self.commit_file("existing.txt", "initial\n")
        existing = self.stage_deletion("existing.txt", "modified staged\n")
        self.assertFalse(new_file.exists())
        self.assertFalse(existing.exists())

        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertTrue(guard.stashed)
            self.assertTrue(new_file.exists())
            self.assertEqual(
                new_file.read_text(encoding="utf-8"), "staged content\n"
            )
            self.assertTrue(existing.exists())
            self.assertEqual(
                existing.read_text(encoding="utf-8"), "modified staged\n"
            )

        self.assertFalse(new_file.exists())
        self.assertFalse(existing.exists())

    def test_stash_guard_rolls_back_working_tree_mutations(self) -> None:
        """Verifies working tree mutations made under guard are discarded before restoring."""
        f = self.stage_partial(
            "file.txt", "staged content\n", "staged and unstaged content\n"
        )

        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertTrue(guard.stashed)
            self.assertEqual(f.read_text(encoding="utf-8"), "staged content\n")
            # Simulate an errant check tool mutating a tracked file on disk
            f.write_text("corrupted by check tool\n", encoding="utf-8")

        # Exiting guard discards dirty modifications and cleanly restores unstaged content
        self.assertEqual(
            f.read_text(encoding="utf-8"), "staged and unstaged content\n"
        )
        self.assertStashEmpty()

    def test_stash_guard_propagates_exceptions_and_restores_stash(self) -> None:
        """Verifies exceptions in context are propagated and unstaged changes are restored."""
        f = self.stage_partial(
            "file.txt", "staged content\n", "staged and unstaged content\n"
        )

        with self.assertRaisesRegex(ValueError, "custom failure"):
            with git_staging.StashGuard(self.test_dir) as guard:
                self.assertTrue(guard.stashed)
                raise ValueError("custom failure")

        self.assertEqual(
            f.read_text(encoding="utf-8"), "staged and unstaged content\n"
        )
        self.assertStashEmpty()

    def test_stash_guard_preserves_pre_existing_stash(self) -> None:
        """Verifies pre-existing stashes are preserved with identical commit SHA."""
        self.write_file(".gitignore", "# user change to stash\n")
        self.git("stash", "push", "-m", "user-existing-stash")
        pre_existing_sha = engine.get_stash_sha(self.test_dir)
        self.assertIsNotNone(pre_existing_sha)

        f = self.stage_partial(
            "file.txt", "staged content\n", "staged and unstaged content\n"
        )

        with git_staging.StashGuard(self.test_dir) as guard:
            self.assertTrue(guard.stashed)
            self.assertNotEqual(guard.stash_sha, pre_existing_sha)

        self.assertEqual(engine.get_stash_sha(self.test_dir), pre_existing_sha)
        stash_list = self.git("stash", "list").stdout
        self.assertIn("user-existing-stash", stash_list)
        self.assertEqual(
            f.read_text(encoding="utf-8"), "staged and unstaged content\n"
        )

    def test_stash_guard_unborn_branch_raises_git_error(self) -> None:
        """Verifies StashGuard raises GitError when invoked on an unborn branch."""
        with tempfile.TemporaryDirectory() as fresh_dir_str:
            fresh_repo = Path(fresh_dir_str)
            subprocess.run(
                ["git", "init", "--template=", "-b", "main"],
                cwd=fresh_repo,
                check=True,
                capture_output=True,
            )
            subprocess.run(
                ["git", "config", "user.email", "test@example.com"],
                cwd=fresh_repo,
                check=True,
                capture_output=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Test User"],
                cwd=fresh_repo,
                check=True,
                capture_output=True,
            )
            f = fresh_repo / "new_file.txt"
            f.write_text("staged content\n", encoding="utf-8")
            subprocess.run(
                ["git", "add", "new_file.txt"], cwd=fresh_repo, check=True
            )
            f.write_text("staged and unstaged\n", encoding="utf-8")

            with self.assertRaises(engine.GitError) as ctx:
                with git_staging.StashGuard(fresh_repo):
                    pass

            self.assertIn(
                "Cannot isolate partial staging on an unborn branch",
                str(ctx.exception),
            )


class RunStagedPipelineTest(GitWorkspaceTestCase):
    """Unit tests for run_staged_pipeline."""

    def test_empty_staging_returns_ok(self) -> None:
        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx, mutating_actions=[fake_formatter]
        )
        self.assertTrue(result.success)
        self.assertEqual(result.restaged_files, ())

    def test_staging_bails_out_on_filtered_extensions(self) -> None:
        self.stage_file("doc.txt", "hello\n")

        action_called = False

        def failing_action(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            nonlocal action_called
            action_called = True
            return False

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[failing_action],
            extensions=[".py"],
        )
        self.assertTrue(result.success)
        self.assertFalse(action_called)
        self.assertEqual(result.restaged_files, ())

    def test_restaging_multiple_files_via_stream(self) -> None:
        f1 = self.stage_file("code1.py", "a=1\n")
        f2 = self.stage_file("code2.py", "b=1\n")

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[fake_formatter],
        )
        self.assertTrue(result.success)
        self.assertCountEqual(result.restaged_files, ["code1.py", "code2.py"])
        self.assertEqual(f1.read_text(encoding="utf-8"), "a = 1\n")
        self.assertEqual(f2.read_text(encoding="utf-8"), "b = 1\n")
        self.assertStaged("code1.py", "a = 1\n")
        self.assertStaged("code2.py", "b = 1\n")

    def test_staged_pipeline_restages_files_with_spaces_and_special_chars(
        self,
    ) -> None:
        file_spaces = "code with spaces.py"
        file_special = "code with [brackets] and unicode_ñ.py"
        f1 = self.stage_file(file_spaces, "a=1\n")
        f2 = self.stage_file(file_special, "b=1\n")

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx, mutating_actions=[fake_formatter]
        )
        self.assertTrue(result.success)
        self.assertCountEqual(
            result.restaged_files, [file_spaces, file_special]
        )
        self.assertEqual(f1.read_text(encoding="utf-8"), "a = 1\n")
        self.assertEqual(f2.read_text(encoding="utf-8"), "b = 1\n")
        self.assertStaged(file_spaces)
        self.assertStaged(file_special)

    def test_pipeline_clean_files_not_restaged(self) -> None:
        self.stage_file("code.py", "x = 1\n")

        noop_called = False

        def noop_action(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            nonlocal noop_called
            noop_called = True
            return True

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[noop_action, fake_formatter],
        )
        self.assertTrue(result.success)
        self.assertTrue(noop_called)
        self.assertEqual(result.restaged_files, ())

    def test_check_only_mode_does_not_mutate_or_restage(self) -> None:
        f = self.stage_file("code.py", "x=1\n")

        ctx = git_staging.ActionContext(
            repo_root=self.test_dir, check_only=True
        )
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[fake_formatter],
            read_only_checks=[],
        )
        self.assertFalse(result.success)
        self.assertEqual(result.restaged_files, ())
        # Content remains unformatted
        self.assertEqual(f.read_text(encoding="utf-8"), "x=1\n")

    def test_check_only_mode_success_on_clean_files(self) -> None:
        f = self.stage_file("code.py", "x = 1\n")

        ctx = git_staging.ActionContext(
            repo_root=self.test_dir, check_only=True
        )
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[fake_formatter],
            read_only_checks=[],
        )
        self.assertTrue(result.success)
        self.assertEqual(result.restaged_files, ())
        self.assertEqual(f.read_text(encoding="utf-8"), "x = 1\n")

    def test_pipeline_fail_preserves_restaged_files_when_check_action_fails(
        self,
    ) -> None:
        self.stage_file("code.py", "x=1\n")

        def failing_check(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            return False

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[fake_formatter],
            read_only_checks=[failing_check],
        )
        self.assertFalse(result.success)
        self.assertEqual(result.restaged_files, ("code.py",))
        self.assertIn("Read-only check failed", result.error)
        self.assertStaged("code.py", "x = 1\n")

    def test_fully_staged_file_runs_read_only_checks_success(self) -> None:
        self.stage_file("code.py", "x=1\n")

        check_called_with: list[tuple[bool, list[str]]] = []

        def passing_check(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            check_called_with.append((ctx.check_only, list(files)))
            return True

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[fake_formatter],
            read_only_checks=[passing_check],
        )
        self.assertTrue(result.success)
        self.assertEqual(result.restaged_files, ("code.py",))
        self.assertEqual(check_called_with, [(True, ["code.py"])])

    def test_partially_staged_file_conflict_reported_without_corruption(
        self,
    ) -> None:
        f = self.stage_partial("code.py", "x=1\n", "x=1\ny=2\n")

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[fake_formatter],
        )
        self.assertFalse(result.success)
        self.assertEqual(result.partial_conflicts, ("code.py",))
        # Working tree was not corrupted
        self.assertEqual(f.read_text(encoding="utf-8"), "x=1\ny=2\n")

    def test_mutating_action_failure_reverts_disk_mutations(self) -> None:
        """Verifies failed mutating actions roll back disk modifications via pathspec stream."""
        f1 = self.stage_file("code.py", "x=1\n")
        f2 = self.stage_file("code with spaces.py", "y=2\n")

        def failing_mutator(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            for rel_path in files:
                (ctx.repo_root / rel_path).write_text(
                    "corrupted\n", encoding="utf-8"
                )
            return False

        ctx = git_staging.ActionContext(repo_root=self.test_dir)
        result = git_staging.run_staged_pipeline(
            context=ctx,
            mutating_actions=[failing_mutator],
        )

        self.assertFalse(result.success)
        self.assertEqual(result.error, "Mutating action failed")
        # Files on disk reverted to match index content via pathspec stream
        self.assertEqual(f1.read_text(encoding="utf-8"), "x=1\n")
        self.assertEqual(f2.read_text(encoding="utf-8"), "y=2\n")
        diff_res = self.git("diff")
        self.assertEqual(diff_res.stdout, "")


if __name__ == "__main__":
    unittest.main()
