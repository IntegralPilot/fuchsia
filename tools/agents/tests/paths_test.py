#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for path and directory discovery logic."""

from __future__ import annotations

import pathlib
import subprocess
import unittest
from unittest import mock

from agents.lib import paths
from agents_testing.base import BaseTestCase


class PathsTest(BaseTestCase):
    """Tests for filesystem paths and directory discovery functions."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_root = self.test_dir

    def test_find_fuchsia_dir_from_tree_jiri_root(self) -> None:
        fake_root = self.mock_root / "workspace_jiri"
        (fake_root / ".jiri_root").mkdir(parents=True)
        sub_file = fake_root / "tools" / "agents" / "lib" / "paths.py"
        sub_file.parent.mkdir(parents=True)
        sub_file.write_text("# placeholder", encoding="utf-8")

        self.patch_object(paths, "__file__", str(sub_file))
        self.patch_environ(clear=True)
        found = paths.find_fuchsia_dir()
        self.assertEqual(found, fake_root)

    def test_find_fuchsia_dir_from_tree_fx_root(self) -> None:
        fake_root = self.mock_root / "workspace_fx"
        fake_root.mkdir(parents=True)
        (fake_root / ".fx-root").touch()
        sub_file = fake_root / "tools" / "agents" / "lib" / "paths.py"
        sub_file.parent.mkdir(parents=True)
        sub_file.write_text("# placeholder", encoding="utf-8")

        self.patch_object(paths, "__file__", str(sub_file))
        self.patch_environ(clear=True)
        found = paths.find_fuchsia_dir()
        self.assertEqual(found, fake_root)

    def test_find_fuchsia_dir_with_start_dir_jiri_root(self) -> None:
        fake_root = self.mock_root / "workspace_start"
        (fake_root / ".jiri_root").mkdir(parents=True)
        nested_dir = fake_root / "a" / "b" / "c"
        nested_dir.mkdir(parents=True)

        found = paths.find_fuchsia_dir(start_dir=nested_dir)
        self.assertEqual(found, fake_root)

    def test_find_fuchsia_dir_with_start_dir_fx_root(self) -> None:
        fake_root = self.mock_root / "workspace_start_fx"
        fake_root.mkdir(parents=True)
        (fake_root / ".fx-root").touch()
        nested_dir = fake_root / "x" / "y" / "z"
        nested_dir.mkdir(parents=True)

        found = paths.find_fuchsia_dir(start_dir=nested_dir)
        self.assertEqual(found, fake_root)

    def test_find_fuchsia_dir_start_dir_miss_raises(self) -> None:
        empty_dir = self.mock_root / "empty_dir"
        empty_dir.mkdir(parents=True)

        fake_root = self.mock_root / "workspace_file"
        (fake_root / ".jiri_root").mkdir(parents=True)
        sub_file = fake_root / "tools" / "agents" / "lib" / "paths.py"
        sub_file.parent.mkdir(parents=True)
        sub_file.write_text("# placeholder", encoding="utf-8")

        self.patch_object(paths, "__file__", str(sub_file))
        self.patch_environ(FUCHSIA_DIR=str(fake_root))
        with self.assertRaises(RuntimeError) as ctx:
            paths.find_fuchsia_dir(start_dir=empty_dir)
        self.assertIn(
            f"Could not locate Fuchsia root directory from start directory: {empty_dir}",
            str(ctx.exception),
        )

    def test_find_fuchsia_dir_env_precedence_over_file(self) -> None:
        fake_root = self.mock_root / "workspace_file"
        (fake_root / ".jiri_root").mkdir(parents=True)
        sub_file = fake_root / "tools" / "agents" / "lib" / "paths.py"
        sub_file.parent.mkdir(parents=True)
        sub_file.write_text("# placeholder", encoding="utf-8")

        target_root = self.mock_root / "env_root"
        target_root.mkdir(parents=True)
        (target_root / ".jiri_root").mkdir(parents=True)

        self.patch_object(paths, "__file__", str(sub_file))
        self.patch_environ(FUCHSIA_DIR=str(target_root))
        found = paths.find_fuchsia_dir()
        self.assertEqual(found, target_root)

    def test_find_fuchsia_dir_ignores_env_without_root_marker(self) -> None:
        fake_root = self.mock_root / "workspace_file"
        (fake_root / ".jiri_root").mkdir(parents=True)
        sub_file = fake_root / "tools" / "agents" / "lib" / "paths.py"
        sub_file.parent.mkdir(parents=True)
        sub_file.write_text("# placeholder", encoding="utf-8")

        # invalid_env lacks .jiri_root and .fx-root
        invalid_env = self.mock_root / "invalid_env"
        invalid_env.mkdir(parents=True)

        self.patch_object(paths, "__file__", str(sub_file))
        self.patch_environ(FUCHSIA_DIR=str(invalid_env))
        # Should ignore FUCHSIA_DIR and resolve from __file__
        found = paths.find_fuchsia_dir()
        self.assertEqual(found, fake_root)

    def test_find_fuchsia_dir_invalid_env_not_found_raises(self) -> None:
        outside_file = self.mock_root / "outside" / "script.py"
        outside_file.parent.mkdir(parents=True)
        outside_file.write_text("# placeholder", encoding="utf-8")
        invalid_env = self.mock_root / "invalid_env"
        invalid_env.mkdir(parents=True)

        self.patch_object(paths, "__file__", str(outside_file))
        self.patch_environ(FUCHSIA_DIR=str(invalid_env))
        with self.assertRaises(RuntimeError) as ctx:
            paths.find_fuchsia_dir()
        self.assertIn(
            "Could not locate Fuchsia root directory",
            str(ctx.exception),
        )

    def test_find_fuchsia_dir_not_found_raises(self) -> None:
        outside_file = self.mock_root / "outside" / "script.py"
        outside_file.parent.mkdir(parents=True)
        outside_file.write_text("# placeholder", encoding="utf-8")

        self.patch_object(paths, "__file__", str(outside_file))
        self.patch_environ(clear=True)
        with self.assertRaises(RuntimeError) as ctx:
            paths.find_fuchsia_dir()
        self.assertIn(
            "Could not locate Fuchsia root directory",
            str(ctx.exception),
        )

    def test_find_config_dirs(self) -> None:
        fuchsia_dir = self.mock_root
        public_cfg = fuchsia_dir / ".agents" / "config"
        public_cfg.mkdir(parents=True, exist_ok=True)
        vendor_google = fuchsia_dir / "vendor" / "google" / ".agents" / "config"
        vendor_google.mkdir(parents=True, exist_ok=True)
        vendor_other = fuchsia_dir / "vendor" / "other" / ".agents" / "config"
        vendor_other.mkdir(parents=True, exist_ok=True)
        # Directory without .agents/config should not be included
        (fuchsia_dir / "vendor" / "novendor").mkdir(parents=True, exist_ok=True)

        config_dirs = paths.find_config_dirs(fuchsia_dir)
        self.assertEqual(
            config_dirs,
            [public_cfg, vendor_google, vendor_other],
        )

    def test_find_config_dirs_no_vendor(self) -> None:
        fuchsia_dir = self.mock_root
        public_cfg = fuchsia_dir / ".agents" / "config"
        public_cfg.mkdir(parents=True, exist_ok=True)

        config_dirs = paths.find_config_dirs(fuchsia_dir)
        self.assertEqual(config_dirs, [public_cfg])

    def test_find_config_dirs_no_root_config(self) -> None:
        fuchsia_dir = self.mock_root
        vendor_google = fuchsia_dir / "vendor" / "google" / ".agents" / "config"
        vendor_google.mkdir(parents=True, exist_ok=True)

        # Root .agents/config does not exist; only vendor config should be returned
        config_dirs = paths.find_config_dirs(fuchsia_dir)
        self.assertEqual(config_dirs, [vendor_google])

    def test_find_config_dirs_empty(self) -> None:
        fuchsia_dir = self.mock_root
        config_dirs = paths.find_config_dirs(fuchsia_dir)
        self.assertEqual(config_dirs, [])

    def test_find_permission_dirs(self) -> None:
        fuchsia_dir = self.mock_root
        public_perm = fuchsia_dir / ".agents" / "config" / "permissions"
        public_perm.mkdir(parents=True, exist_ok=True)
        vendor_perm = (
            fuchsia_dir
            / "vendor"
            / "google"
            / ".agents"
            / "config"
            / "permissions"
        )
        vendor_perm.mkdir(parents=True, exist_ok=True)
        # Vendor config without permissions directory
        vendor_no_perm = fuchsia_dir / "vendor" / "other" / ".agents" / "config"
        vendor_no_perm.mkdir(parents=True, exist_ok=True)

        perm_dirs = paths.find_permission_dirs(fuchsia_dir)
        self.assertEqual(perm_dirs, [public_perm, vendor_perm])

    def test_find_permission_dirs_empty(self) -> None:
        fuchsia_dir = self.mock_root
        public_cfg = fuchsia_dir / ".agents" / "config"
        public_cfg.mkdir(parents=True, exist_ok=True)

        perm_dirs = paths.find_permission_dirs(fuchsia_dir)
        self.assertEqual(perm_dirs, [])

    def test_find_checkout_git_repos_discovers_all(self) -> None:
        fuchsia_dir = self.mock_root
        (fuchsia_dir / ".git").mkdir(parents=True, exist_ok=True)
        vendor_google = fuchsia_dir / "vendor" / "google"
        (vendor_google / ".git").mkdir(parents=True, exist_ok=True)
        vendor_partner = fuchsia_dir / "vendor" / "partner"
        (vendor_partner / ".git").mkdir(parents=True, exist_ok=True)
        integration_dir = fuchsia_dir / "integration"
        (integration_dir / ".git").mkdir(parents=True, exist_ok=True)

        repos = paths.find_checkout_git_repos(fuchsia_dir)
        self.assertEqual(
            repos,
            [fuchsia_dir, vendor_google, vendor_partner, integration_dir],
        )

    def test_find_checkout_git_repos_root_only(self) -> None:
        fuchsia_dir = self.mock_root
        (fuchsia_dir / ".git").mkdir(parents=True, exist_ok=True)

        repos = paths.find_checkout_git_repos(fuchsia_dir)
        self.assertEqual(repos, [fuchsia_dir])

    def test_find_checkout_git_repos_discovers_vendor_and_integration(
        self,
    ) -> None:
        fuchsia_dir = self.mock_root
        vendor_google = fuchsia_dir / "vendor" / "google"
        (vendor_google / ".git").mkdir(parents=True, exist_ok=True)
        vendor_partner = fuchsia_dir / "vendor" / "partner"
        (vendor_partner / ".git").mkdir(parents=True, exist_ok=True)
        integration_dir = fuchsia_dir / "integration"
        (integration_dir / ".git").mkdir(parents=True, exist_ok=True)

        repos = paths.find_checkout_git_repos(fuchsia_dir)
        self.assertEqual(
            repos,
            [vendor_google, vendor_partner, integration_dir],
        )

    def test_find_checkout_git_repos_ignores_non_git_dirs(self) -> None:
        fuchsia_dir = self.mock_root
        (fuchsia_dir / "vendor" / "novendor").mkdir(parents=True, exist_ok=True)
        (fuchsia_dir / "vendor" / "README.md").write_text(
            "info", encoding="utf-8"
        )
        (fuchsia_dir / "integration").mkdir(parents=True, exist_ok=True)
        (fuchsia_dir / "src").mkdir(parents=True, exist_ok=True)

        repos = paths.find_checkout_git_repos(fuchsia_dir)
        self.assertEqual(repos, [])

    def test_find_checkout_git_repos_git_file(self) -> None:
        fuchsia_dir = self.mock_root
        (fuchsia_dir / ".git").touch()
        vendor_google = fuchsia_dir / "vendor" / "google"
        vendor_google.mkdir(parents=True, exist_ok=True)
        (vendor_google / ".git").touch()
        integration_dir = fuchsia_dir / "integration"
        integration_dir.mkdir(parents=True, exist_ok=True)
        (integration_dir / ".git").touch()

        repos = paths.find_checkout_git_repos(fuchsia_dir)
        self.assertEqual(
            repos,
            [fuchsia_dir, vendor_google, integration_dir],
        )

    @mock.patch("subprocess.run")
    def test_git_rev_parse_success(self, mock_run: mock.MagicMock) -> None:
        mock_run.return_value = subprocess.CompletedProcess(
            args=["git", "rev-parse", "--git-dir"],
            returncode=0,
            stdout=".git\n",
            stderr="",
        )
        res = paths._git_rev_parse(self.mock_root, "--git-dir")
        self.assertEqual(res, ".git")
        mock_run.assert_called_once_with(
            ["git", "rev-parse", "--git-dir"],
            cwd=self.mock_root,
            capture_output=True,
            text=True,
        )

    @mock.patch("subprocess.run")
    def test_git_rev_parse_failure(self, mock_run: mock.MagicMock) -> None:
        mock_run.return_value = subprocess.CompletedProcess(
            args=["git", "rev-parse", "--git-dir"],
            returncode=128,
            stdout="",
            stderr="fatal: not a git repo",
        )
        with self.assertRaises(RuntimeError) as ctx:
            paths._git_rev_parse(self.mock_root, "--git-dir")
        self.assertIn(
            "Directory is not in a git repository", str(ctx.exception)
        )

    @mock.patch.object(paths, "_git_rev_parse")
    def test_get_git_dir_and_hooks_dir(
        self, mock_rev_parse: mock.MagicMock
    ) -> None:
        repo_dir = self.mock_root / "real_repo"
        repo_dir.mkdir()

        def side_effect(cwd: pathlib.Path, *args: str) -> str:
            if args == ("--git-dir",):
                return ".git"
            if args == ("--git-path", "hooks"):
                return ".git/hooks"
            raise RuntimeError(f"Unexpected args: {args}")

        mock_rev_parse.side_effect = side_effect

        git_dir = paths.get_git_dir(repo_dir)
        hooks_dir = paths.get_hooks_dir(repo_dir)

        self.assertEqual(git_dir, (repo_dir / ".git").resolve())
        self.assertEqual(hooks_dir, (repo_dir / ".git" / "hooks").resolve())

    @mock.patch.object(paths, "_git_rev_parse")
    def test_get_git_dir_and_hooks_dir_worktree(
        self, mock_rev_parse: mock.MagicMock
    ) -> None:
        repo_dir = self.mock_root / "real_repo_wt"
        repo_dir.mkdir()
        wt_dir = self.mock_root / "worktree"
        wt_dir.mkdir()

        wt_git_dir = repo_dir / ".git" / "worktrees" / "wt"
        common_hooks = repo_dir / ".git" / "hooks"

        def side_effect(cwd: pathlib.Path, *args: str) -> str:
            if args == ("--git-dir",):
                return str(wt_git_dir)
            if args == ("--git-path", "hooks"):
                return str(common_hooks)
            raise RuntimeError(f"Unexpected args: {args}")

        mock_rev_parse.side_effect = side_effect

        git_dir = paths.get_git_dir(wt_dir)
        hooks_dir = paths.get_hooks_dir(wt_dir)

        self.assertTrue(git_dir.is_absolute())
        self.assertEqual(git_dir, wt_git_dir.resolve())
        self.assertEqual(hooks_dir, common_hooks.resolve())

    @mock.patch.object(paths, "_git_rev_parse")
    def test_get_git_dir_and_hooks_dir_non_repo(
        self, mock_rev_parse: mock.MagicMock
    ) -> None:
        non_repo = self.mock_root / "non_repo"
        non_repo.mkdir()
        mock_rev_parse.side_effect = RuntimeError(
            f"Directory is not in a git repository: {non_repo}"
        )

        with self.assertRaises(RuntimeError) as ctx:
            paths.get_git_dir(non_repo)
        self.assertIn(
            "Directory is not in a git repository", str(ctx.exception)
        )

        with self.assertRaises(RuntimeError) as ctx2:
            paths.get_hooks_dir(non_repo)
        self.assertIn(
            "Directory is not in a git repository", str(ctx2.exception)
        )

    @mock.patch.object(paths, "_git_rev_parse")
    def test_get_repo_root(self, mock_rev_parse: mock.MagicMock) -> None:
        repo_dir = self.mock_root / "real_repo_for_root"
        repo_dir.mkdir()
        sub_dir = repo_dir / "subdir" / "nested"
        sub_dir.mkdir(parents=True)
        mock_rev_parse.return_value = str(repo_dir)

        self.assertEqual(paths.get_repo_root(sub_dir), repo_dir.resolve())
        mock_rev_parse.assert_called_once_with(sub_dir, "--show-toplevel")

    @mock.patch.object(paths, "_git_rev_parse")
    def test_get_repo_root_not_in_repo(
        self, mock_rev_parse: mock.MagicMock
    ) -> None:
        non_repo = self.mock_root / "non_repo_root"
        non_repo.mkdir()
        mock_rev_parse.side_effect = RuntimeError(
            f"Directory is not in a git repository: {non_repo}"
        )
        with self.assertRaises(RuntimeError) as ctx:
            paths.get_repo_root(non_repo)
        self.assertIn(
            "Directory is not in a git repository", str(ctx.exception)
        )


if __name__ == "__main__":
    unittest.main()
