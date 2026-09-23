#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import subprocess
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))
import get_git_head_commit as gghc
from git_test_utils import git_cmd, git_init


def _setup_first_git_dir(git_dir: Path) -> tuple[str, str]:
    git_init(git_dir, branch="main")
    (git_dir / "foo").write_text("1")
    git_cmd(git_dir, ["add", "foo"])
    git_cmd(git_dir, ["commit", "-m", "first commit"])
    first_commit = git_cmd(git_dir, ["rev-parse", "HEAD"])
    (git_dir / "foo").write_text("2")
    git_cmd(git_dir, ["commit", "-am", "second commit"])
    second_commit = git_cmd(git_dir, ["rev-parse", "HEAD"])
    return (first_commit, second_commit)


def _setup_second_git_dir(git_dir: Path, submodule_path: Path) -> str:
    git_init(git_dir, branch="main")

    # Add a submodule. Using -c protocol.file.allow=always
    # is required to avoid an error message that says:
    # "fatal: transport 'file' not allowed".
    # See https://github.com/flatpak/flatpak-builder/issues/495#issuecomment-1297413908
    git_cmd(
        git_dir,
        [
            "-c",
            "init.defaultRefFormat=files",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            submodule_path,
            "sub1",
        ],
    )
    git_cmd(git_dir, ["commit", "-m", "first commit"])
    return git_cmd(git_dir, ["rev-parse", "HEAD"])


class NoSubmodulesTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._dir = Path(self._td.name)
        self._first_commit, self._second_commit = _setup_first_git_dir(
            self._dir
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def _cmd(self, args: T.Sequence[str]) -> subprocess.CompletedProcess[str]:
        ret = subprocess.run(
            args, cwd=self._dir, capture_output=True, text=True
        )
        if ret.returncode != 0:
            print(ret.stdout)
            print(ret.stderr, file=sys.stderr)
            ret.check_returncode()

        return ret

    def _cmd_output(self, args: T.Sequence[str]) -> str:
        ret = self._cmd(args)
        return ret.stdout.strip()

    def test_on_branch(self) -> None:
        current_head = gghc.get_git_head_commit(self._dir)
        self.assertEqual(current_head, self._second_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in sorted(gghc.find_git_head_inputs(self._dir))
        )
        self.assertListEqual(
            input_files, [".git/HEAD", ".git/config", ".git/refs/heads/main"]
        )

    def test_on_branch_with_packed_refs(self) -> None:
        self._cmd(["git", "pack-refs", "--all"])

        current_head = gghc.get_git_head_commit(self._dir)
        self.assertEqual(current_head, self._second_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in sorted(gghc.find_git_head_inputs(self._dir))
        )
        self.assertListEqual(
            input_files, [".git/HEAD", ".git/config", ".git/packed-refs"]
        )

    def test_detached_state(self) -> None:
        self._cmd(["git", "checkout", "main^"])
        current_head = gghc.get_git_head_commit(self._dir)
        self.assertEqual(current_head, self._first_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in sorted(gghc.find_git_head_inputs(self._dir))
        )
        self.assertListEqual(input_files, [".git/HEAD", ".git/config"])


class WithSubmodulesTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._top_dir = Path(self._td.name)
        self._first_dir = self._top_dir / "first_dir"
        self._first_commit, self._second_commit = _setup_first_git_dir(
            self._first_dir
        )

        self._dir = self._top_dir / "second_dir"
        self._first_commit2 = _setup_second_git_dir(self._dir, self._first_dir)

    def tearDown(self) -> None:
        self._td.cleanup()

    def _cmd(self, args: T.Sequence[str]) -> subprocess.CompletedProcess[str]:
        ret = subprocess.run(
            args, cwd=self._dir, capture_output=True, text=True
        )
        if ret.returncode != 0:
            print(ret.stdout)
            print(ret.stderr, file=sys.stderr)
            ret.check_returncode()

        return ret

    def _cmd_output(self, args: T.Sequence[str]) -> str:
        ret = self._cmd(args)
        return ret.stdout.strip()

    def test_on_branch(self) -> None:
        git_dir = self._dir / "sub1"
        current_head = gghc.get_git_head_commit(git_dir)
        self.assertEqual(current_head, self._second_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in gghc.find_git_head_inputs(git_dir)
        )
        # For some reason, all sub-modules seem to have a packed-refs
        # file which is accessed unconditionally when the branch ref file
        # exists.
        self.assertListEqual(
            input_files,
            [
                ".git/modules/sub1/HEAD",
                ".git/modules/sub1/config",
                ".git/modules/sub1/packed-refs",
                ".git/modules/sub1/refs/heads/main",
                "sub1/.git",
            ],
        )

    def test_on_branch_with_packed_refs(self) -> None:
        git_dir = self._dir / "sub1"
        self._cmd(["git", "-C", str(git_dir), "pack-refs", "--all"])

        current_head = gghc.get_git_head_commit(git_dir)
        self.assertEqual(current_head, self._second_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in gghc.find_git_head_inputs(git_dir)
        )
        self.assertListEqual(
            input_files,
            [
                ".git/modules/sub1/HEAD",
                ".git/modules/sub1/config",
                ".git/modules/sub1/packed-refs",
                "sub1/.git",
            ],
        )

    def test_detached_state(self) -> None:
        git_dir = self._dir / "sub1"
        self._cmd(["git", "-C", str(git_dir), "checkout", "main^"])
        current_head = gghc.get_git_head_commit(self._dir / "sub1")
        self.assertEqual(current_head, self._first_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in gghc.find_git_head_inputs(git_dir)
        )
        # For some reason, all sub-modules seem to have a packed-refs
        # file which is accessed unconditionally when in detached head.
        self.assertListEqual(
            input_files,
            [
                ".git/modules/sub1/HEAD",
                ".git/modules/sub1/config",
                ".git/modules/sub1/packed-refs",
                "sub1/.git",
            ],
        )


class WithWorktreeTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._top_dir = Path(self._td.name)
        self._base_dir = self._top_dir / "base"
        self._first_commit, self._second_commit = _setup_first_git_dir(
            self._base_dir
        )
        self._dir = self._top_dir / "worktree"

        # Create worktree on a new branch.
        # We use a unique branch name to avoid conflicts.
        git_cmd(
            self._base_dir, ["worktree", "add", self._dir, "-b", "wt-branch"]
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def _cmd(self, args: T.Sequence[str]) -> "subprocess.CompletedProcess[str]":
        return subprocess.run(
            args, cwd=self._dir, capture_output=True, text=True
        )

    def test_on_branch(self) -> None:
        current_head = gghc.get_git_head_commit(self._dir)
        self.assertEqual(current_head, self._second_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in gghc.find_git_head_inputs(self._dir)
        )

        # We expect:
        # 1. The .git file in the worktree.
        # 2. The shared config in the base repo.
        # 3. The worktree-specific HEAD.
        # 4. The shared branch ref in the base repo.
        expected = [
            ".git",
            "../base/.git/config",
            "../base/.git/worktrees/worktree/HEAD",
            "../base/.git/refs/heads/wt-branch",
        ]
        expected_rel = sorted(os.path.normpath(p) for p in expected)

        self.assertListEqual(input_files, expected_rel)

    def test_detached_state(self) -> None:
        # Checkout a detached head in the worktree.
        # Note: We must use -d or --detach, and we must not use a branch that is checked out elsewhere.
        # We can checkout 'main^' (which is the first commit).
        self._cmd(["git", "checkout", "--detach", "main^"])
        current_head = gghc.get_git_head_commit(self._dir)
        self.assertEqual(current_head, self._first_commit)

        input_files = sorted(
            os.path.relpath(f, self._dir)
            for f in gghc.find_git_head_inputs(self._dir)
        )

        # In detached state, we expect:
        # 1. The .git file.
        # 2. The shared config.
        # 3. The worktree-specific HEAD.
        # (No branch ref)
        expected = [
            ".git",
            "../base/.git/config",
            "../base/.git/worktrees/worktree/HEAD",
        ]
        expected_rel = sorted(os.path.normpath(p) for p in expected)
        self.assertListEqual(input_files, expected_rel)


if __name__ == "__main__":
    unittest.main()
