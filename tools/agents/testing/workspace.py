# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Hermetic Git workspace test case using fast template caching."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from agents_testing.base import BaseTestCase


@unittest.skipUnless(
    shutil.which("git") is not None,
    "git binary not available in test environment",
)
class GitWorkspaceTestCase(BaseTestCase):
    """Hermetic Git workspace test case using fast template repository caching."""

    _template_dir: tempfile.TemporaryDirectory[str] | None = None
    _template_path: Path | None = None

    @classmethod
    def setUpClass(cls) -> None:
        if not shutil.which("git"):
            raise unittest.SkipTest(
                "git binary not available in test environment"
            )
        super().setUpClass()
        cls._template_dir = tempfile.TemporaryDirectory()
        cls._template_path = Path(cls._template_dir.name)
        cls.init_pristine_git_repo(cls._template_path, is_fuchsia_root=True)

    @classmethod
    def tearDownClass(cls) -> None:
        if cls._template_dir is not None:
            cls._template_dir.cleanup()
            cls._template_dir = None
            cls._template_path = None
        super().tearDownClass()

    @classmethod
    def init_pristine_git_repo(
        cls, repo_dir: Path, is_fuchsia_root: bool = False
    ) -> None:
        """Initializes a pristine Git repository with hermetic settings."""
        env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        subprocess.run(
            ["git", "init", "--template=", "-b", "main"],
            cwd=repo_dir,
            check=True,
            capture_output=True,
            env=env,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test User"],
            cwd=repo_dir,
            check=True,
            capture_output=True,
            env=env,
        )
        subprocess.run(
            ["git", "config", "user.email", "test@example.com"],
            cwd=repo_dir,
            check=True,
            capture_output=True,
            env=env,
        )
        subprocess.run(
            ["git", "config", "commit.gpgsign", "false"],
            cwd=repo_dir,
            check=True,
            capture_output=True,
            env=env,
        )
        (repo_dir / ".gitignore").write_text("# git init\n", encoding="utf-8")
        add_files = [".gitignore"]
        if is_fuchsia_root:
            (repo_dir / ".fx-root").touch()
            add_files.append(".fx-root")
        subprocess.run(
            ["git", "add", *add_files],
            cwd=repo_dir,
            check=True,
            capture_output=True,
            env=env,
        )
        subprocess.run(
            ["git", "commit", "-m", "initial commit"],
            cwd=repo_dir,
            check=True,
            capture_output=True,
            env=env,
        )

    def setUp(self) -> None:
        super().setUp()
        assert self._template_path is not None
        # Fast copy of template repo into isolated test_dir
        shutil.copytree(
            self._template_path,
            self.test_dir,
            dirs_exist_ok=True,
            symlinks=True,
        )

    def git(
        self,
        *args: str,
        check: bool = True,
        capture_output: bool = True,
        cwd: Path | None = None,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        """Convenience wrapper to run git commands in self.test_dir."""
        git_env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            **(env or {}),
        }
        return subprocess.run(
            ["git", *args],
            cwd=cwd or self.test_dir,
            check=check,
            capture_output=capture_output,
            text=True,
            env=git_env,
        )

    def commit_file(
        self, rel_path: str | Path, content: str = "", msg: str = "test commit"
    ) -> Path:
        """Writes a file, stages it with git add, and commits it."""
        file_path = self.write_file(rel_path, content)
        self.git("add", str(rel_path))
        self.git("commit", "-m", msg)
        return file_path

    def create_worktree(
        self, worktree_dir: Path, branch: str = "worktree-branch"
    ) -> Path:
        """Creates a linked git worktree in worktree_dir."""
        self.git("worktree", "add", str(worktree_dir), "-b", branch)
        return worktree_dir

    def stage_file(self, rel_path: str | Path, content: str = "") -> Path:
        """Writes a file and stages it to the git index."""
        file_path = self.write_file(rel_path, content)
        self.git("add", str(rel_path))
        return file_path

    def stage_partial(
        self, rel_path: str | Path, staged_content: str, unstaged_content: str
    ) -> Path:
        """Creates a partially staged file in the working tree."""
        file_path = self.stage_file(rel_path, staged_content)
        file_path.write_text(unstaged_content, encoding="utf-8")
        return file_path

    def stage_deletion(
        self, rel_path: str | Path, staged_content: str = "staged"
    ) -> Path:
        """Stages a file and unlinks it from the working tree."""
        file_path = self.stage_file(rel_path, staged_content)
        file_path.unlink()
        return file_path

    def create_subrepo(self, rel_path: str | Path) -> Path:
        """Initializes a pristine git repository at a subpath."""
        subrepo_dir = self.test_dir / rel_path
        subrepo_dir.mkdir(parents=True, exist_ok=True)
        self.init_pristine_git_repo(subrepo_dir, is_fuchsia_root=False)
        return subrepo_dir

    def assertStaged(
        self, rel_path: str | Path, expected_content: str | None = None
    ) -> None:
        """Asserts that a file is staged in the index."""
        diff = [
            p
            for p in self.git(
                "diff", "--cached", "-z", "--name-only"
            ).stdout.split("\0")
            if p
        ]
        self.assertIn(str(rel_path), diff)
        if expected_content is not None:
            show = self.git("show", f":{rel_path}").stdout
            self.assertEqual(show, expected_content)

    def assertStashEmpty(self) -> None:
        """Asserts that the git stash stack is empty."""
        stash_list = self.git("stash", "list").stdout.strip()
        self.assertEqual(stash_list, "")
