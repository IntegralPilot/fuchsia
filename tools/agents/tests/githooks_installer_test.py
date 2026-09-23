#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import unittest
from pathlib import Path
from unittest import mock

from agents.lib.githooks import HookOperationResult, installer
from agents_testing.workspace import GitWorkspaceTestCase


class GitHooksInstallerTest(GitWorkspaceTestCase):
    def hook_fragment_path(
        self,
        hook_name: str,
        filename: str = "10-fuchsia-agent.sh",
        repo_dir: Path | None = None,
    ) -> Path:
        """Returns the canonical path to a hook fragment."""
        root = repo_dir or self.test_dir
        return root / ".git" / "hooks" / f"{hook_name}.d" / filename

    def test_install_git_hook(self) -> None:
        script_path = "/path/to/script.py"
        args = ["pre-commit"]
        installed = installer.install_git_hook(
            self.test_dir,
            hook_name="pre-commit",
            script_path=script_path,
            script_args=args,
            dry_run=False,
        )
        hook_path = self.hook_fragment_path("pre-commit")
        self.assertEqual(installed, hook_path)
        self.assertTrue(hook_path.is_file())

        content = hook_path.read_text(encoding="utf-8")
        self.assertIn("exec", content)
        self.assertIn(script_path, content)
        self.assertIn("pre-commit", content)

    def test_install_git_hook_dry_run(self) -> None:
        script_path = "/path/to/script.py"
        installed = installer.install_git_hook(
            self.test_dir,
            hook_name="pre-commit",
            script_path=script_path,
            dry_run=True,
        )
        hook_path = self.hook_fragment_path("pre-commit")
        self.assertEqual(installed, hook_path)
        self.assertFalse(hook_path.exists())

    def test_uninstall_git_hook(self) -> None:
        script_path = "/path/to/script.py"
        installer.install_git_hook(
            self.test_dir,
            hook_name="pre-commit",
            script_path=script_path,
            dry_run=False,
        )

        removed = installer.uninstall_git_hook(
            self.test_dir, hook_name="pre-commit", dry_run=False
        )
        hook_path = self.hook_fragment_path("pre-commit")
        self.assertEqual(removed, hook_path)
        self.assertFalse(hook_path.exists())

    def test_uninstall_git_hook_dry_run(self) -> None:
        script_path = "/path/to/script.py"
        installer.install_git_hook(
            self.test_dir,
            hook_name="pre-commit",
            script_path=script_path,
            dry_run=False,
        )

        removed = installer.uninstall_git_hook(
            self.test_dir, hook_name="pre-commit", dry_run=True
        )
        hook_path = self.hook_fragment_path("pre-commit")
        self.assertEqual(removed, hook_path)
        self.assertTrue(hook_path.exists())

    def test_uninstall_git_hook_not_installed(self) -> None:
        removed = installer.uninstall_git_hook(
            self.test_dir, hook_name="pre-commit", dry_run=False
        )
        self.assertIsNone(removed)

    def test_create_subrepo_does_not_contain_fx_root(self) -> None:
        subrepo = self.create_subrepo("vendor/google")
        self.assertTrue((subrepo / ".git").is_dir())
        self.assertFalse((subrepo / ".fx-root").exists())
        self.assertTrue((self.test_dir / ".fx-root").exists())

    def test_multi_repo_install_uninstall_status(self) -> None:
        # 1. Status starts empty
        status = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status.total_repos, 1)
        self.assertEqual(status.configured_repos, 0)

        # 2. Add vendor repo
        vendor_repo = self.create_subrepo("vendor/google")
        self.assertFalse((vendor_repo / ".fx-root").exists())
        status = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status.total_repos, 2)
        self.assertEqual(status.configured_repos, 0)

        # 3. Configure all
        installed = installer.install_git_hooks(self.test_dir, dry_run=False)
        self.assertIsInstance(installed, HookOperationResult)
        self.assertEqual(len(installed.modified), 4)
        status = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status.total_repos, 2)
        self.assertEqual(status.configured_repos, 2)

        # 4. Uninstall from vendor repo manually to test partial status
        installer.uninstall_git_hook(vendor_repo, hook_name="pre-commit")
        status = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status.total_repos, 2)
        self.assertEqual(status.configured_repos, 1)

        # 5. Uninstall all
        uninstalled = installer.uninstall_git_hooks(
            self.test_dir, dry_run=False
        )
        self.assertIsInstance(uninstalled, HookOperationResult)
        self.assertEqual(
            len(uninstalled.modified), 3
        )  # Only root repo had 2 left, vendor had 1 left
        status = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status.total_repos, 2)
        self.assertEqual(status.configured_repos, 0)

    def test_get_git_hooks_status_all_vs_partial(self) -> None:
        status = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status.total_repos, 1)
        self.assertEqual(status.configured_repos, 0)

        # Install only pre-commit
        installer.install_git_hook(
            self.test_dir,
            hook_name="pre-commit",
            script_path=self.test_dir / "runner.py",
        )
        # Default expects both pre-commit and commit-msg -> partial returns 0
        status_default = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status_default.configured_repos, 0)

        # Checking only pre-commit returns 1
        status_pre = installer.get_git_hooks_status(
            self.test_dir, hook_names=["pre-commit"]
        )
        self.assertEqual(status_pre.configured_repos, 1)

        # Install commit-msg as well
        installer.install_git_hook(
            self.test_dir,
            hook_name="commit-msg",
            script_path=self.test_dir / "runner.py",
        )
        # Both are now installed -> default returns 1
        status_both = installer.get_git_hooks_status(self.test_dir)
        self.assertEqual(status_both.configured_repos, 1)

    def test_install_git_hooks_runtime_error_resilience(self) -> None:
        non_git_dir = self.test_dir / "non_git"
        with mock.patch(
            "agents.lib.githooks.installer.find_checkout_git_repos",
            return_value=[self.test_dir, non_git_dir],
        ):
            installed = installer.install_git_hooks(
                self.test_dir,
                hook_names=["pre-commit", "commit-msg"],
                dry_run=False,
            )
            root_fragment = self.hook_fragment_path("pre-commit")
            self.assertEqual(len(installed.modified), 2)
            self.assertIn(root_fragment, installed.modified)
            self.assertEqual(len(installed.failed), 1)
            self.assertEqual(installed.failed[0][0], non_git_dir)
            self.assertTrue(installed.failed[0][1])
            self.assertTrue(root_fragment.is_file())

    def test_uninstall_git_hooks_runtime_error_resilience(self) -> None:
        installer.install_git_hooks(
            self.test_dir,
            hook_names=["pre-commit", "commit-msg"],
            dry_run=False,
        )
        root_fragment = self.hook_fragment_path("pre-commit")
        self.assertTrue(root_fragment.is_file())

        non_git_dir = self.test_dir / "non_git"
        with mock.patch(
            "agents.lib.githooks.installer.find_checkout_git_repos",
            return_value=[self.test_dir, non_git_dir],
        ):
            uninstalled = installer.uninstall_git_hooks(
                self.test_dir,
                hook_names=["pre-commit", "commit-msg"],
                dry_run=False,
            )
            self.assertEqual(len(uninstalled.modified), 2)
            self.assertIn(root_fragment, uninstalled.modified)
            self.assertEqual(len(uninstalled.failed), 1)
            self.assertEqual(uninstalled.failed[0][0], non_git_dir)
            self.assertTrue(uninstalled.failed[0][1])
            self.assertFalse(root_fragment.exists())

    def test_get_git_hooks_status_runtime_error_resilience(self) -> None:
        installer.install_git_hooks(self.test_dir, dry_run=False)
        non_git_dir = self.test_dir / "non_git"
        with mock.patch(
            "agents.lib.githooks.installer.find_checkout_git_repos",
            return_value=[self.test_dir, non_git_dir],
        ):
            status = installer.get_git_hooks_status(self.test_dir)
            self.assertEqual(status.total_repos, 2)
            self.assertEqual(status.configured_repos, 1)


if __name__ == "__main__":
    unittest.main()
