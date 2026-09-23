# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for preflight."""

import subprocess
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

import cartfs
import preflight
import workspace


class TestPreflight(unittest.TestCase):
    """Tests for preflight."""

    @patch("subprocess.check_call")
    def test_check_gcert_status_success(
        self, mock_check_call: MagicMock
    ) -> None:
        """Test that check_gcert_status returns True on success."""
        self.assertTrue(preflight.check_gcert_status())
        mock_check_call.assert_called_with(
            ["gcertstatus", "-check_ssh=false", "-quiet"]
        )

    @patch(
        "subprocess.check_call",
        side_effect=subprocess.CalledProcessError(1, "cmd"),
    )
    def test_check_gcert_status_failure(
        self, mock_check_call: MagicMock
    ) -> None:
        """Test that check_gcert_status returns False on failure."""
        self.assertFalse(preflight.check_gcert_status())

    @patch("shutil.which", return_value=None)
    def test_check_git_citc_cogd_no_git(self, mock_which: MagicMock) -> None:
        """Test that check_git_citc_cogd returns False when git is missing."""
        self.assertFalse(preflight.check_git_citc_cogd())

    @patch("shutil.which", return_value="/usr/bin/git")
    @patch.object(workspace.Workspace, "cogd_path")
    def test_check_git_citc_cogd_success(
        self, mock_cogd_path: MagicMock, mock_which: MagicMock
    ) -> None:
        """Test that check_git_citc_cogd returns True on success."""
        mock_cogd_path.return_value = Path("/path/to/cogd")
        self.assertTrue(preflight.check_git_citc_cogd())

    @patch("shutil.which")
    def test_check_git_citc_cogd_no_git_citc(
        self, mock_which: MagicMock
    ) -> None:
        """Test that check_git_citc_cogd returns False when git-citc is missing."""
        mock_which.side_effect = (
            lambda x: "/usr/bin/git" if x == "git" else None
        )
        self.assertFalse(preflight.check_git_citc_cogd())

    @patch("shutil.which", return_value="/usr/bin/git")
    @patch.object(
        workspace.Workspace,
        "cogd_path",
        side_effect=workspace.NotInCogWorkspaceError("mock error"),
    )
    def test_check_git_citc_cogd_not_in_workspace(
        self, mock_cogd_path: MagicMock, mock_which: MagicMock
    ) -> None:
        """Test that check_git_citc_cogd returns False when not in workspace."""
        self.assertFalse(preflight.check_git_citc_cogd())

    @patch("shutil.which", return_value=None)
    def test_check_cartfs_no_cartfs(self, mock_which: MagicMock) -> None:
        """Test that check_cartfs returns False when cartfs is missing."""
        self.assertFalse(preflight.check_cartfs())

    @patch("shutil.which")
    @patch.object(cartfs.Cartfs, "cartfs_uid", return_value=111)
    @patch.object(
        cartfs.Cartfs, "find_mount_point", return_value="/mount/point"
    )
    def test_check_cartfs_success(
        self,
        mock_find_mount: MagicMock,
        mock_uid: MagicMock,
        mock_which: MagicMock,
    ) -> None:
        """Test that check_cartfs returns True on success."""
        mock_which.side_effect = lambda x: "/usr/bin/" + x
        self.assertTrue(preflight.check_cartfs())

    @patch("shutil.which")
    @patch.object(
        cartfs.Cartfs,
        "cartfs_uid",
        side_effect=cartfs.CartfsNotRunningError("mock error"),
    )
    def test_check_cartfs_uid_error(
        self, mock_uid: MagicMock, mock_which: MagicMock
    ) -> None:
        """Test that check_cartfs returns False on UID error."""
        mock_which.side_effect = lambda x: "/usr/bin/" + x
        self.assertFalse(preflight.check_cartfs())

    @patch(
        "shutil.which",
        side_effect=lambda x: "/usr/bin/" + x if x == "cartfs" else None,
    )
    def test_check_cartfs_no_grpc_cli(self, mock_which: MagicMock) -> None:
        """Test that check_cartfs returns False when grpc_cli is missing."""
        self.assertFalse(preflight.check_cartfs())

    @patch("shutil.which")
    @patch.object(cartfs.Cartfs, "cartfs_uid", return_value=111)
    @patch.object(
        cartfs.Cartfs, "find_mount_point", return_value="/mount/point"
    )
    def test_check_cartfs_no_grpc_cli_not_required(
        self,
        mock_find_mount: MagicMock,
        mock_uid: MagicMock,
        mock_which: MagicMock,
    ) -> None:
        """Test that check_cartfs returns True when grpc_cli is missing but not required."""
        mock_which.side_effect = (
            lambda x: "/usr/bin/" + x if x == "cartfs" else None
        )
        self.assertTrue(preflight.check_cartfs(require_grpc_cli=False))

    @patch.object(preflight, "check_gcert_status", return_value=True)
    @patch.object(preflight, "check_git_citc_cogd", return_value=True)
    @patch.object(preflight, "check_cartfs", return_value=True)
    def test_check_all_success(
        self, mock_cartfs: MagicMock, mock_git: MagicMock, mock_gcert: MagicMock
    ) -> None:
        """Test that check_all returns True on success."""
        self.assertTrue(preflight.check_all())

    @patch.object(preflight, "check_gcert_status", return_value=False)
    def test_check_all_gcert_fail(self, mock_gcert: MagicMock) -> None:
        """Test that check_all returns False on gcert failure."""
        self.assertFalse(preflight.check_all())

    @patch.object(preflight, "check_gcert_status", return_value=True)
    @patch.object(preflight, "check_git_citc_cogd", return_value=False)
    def test_check_all_git_fail(
        self, mock_git: MagicMock, mock_gcert: MagicMock
    ) -> None:
        """Test that check_all returns False on git failure."""
        self.assertFalse(preflight.check_all())

    @patch.object(preflight, "check_gcert_status", return_value=True)
    @patch.object(preflight, "check_git_citc_cogd", return_value=True)
    @patch.object(preflight, "check_cartfs", return_value=False)
    def test_check_all_cartfs_fail(
        self, mock_cartfs: MagicMock, mock_git: MagicMock, mock_gcert: MagicMock
    ) -> None:
        """Test that check_all returns False on cartfs failure."""
        self.assertFalse(preflight.check_all())

    @patch.object(preflight, "check_gcert_status", return_value=True)
    @patch.object(preflight, "check_git_citc_cogd", return_value=True)
    @patch.object(preflight, "check_cartfs")
    def test_check_all_require_grpc_cli_false(
        self,
        mock_check_cartfs: MagicMock,
        mock_git: MagicMock,
        mock_gcert: MagicMock,
    ) -> None:
        """Test that check_all passes require_grpc_cli=False to check_cartfs."""
        preflight.check_all(require_grpc_cli=False)
        mock_check_cartfs.assert_called_once_with(False)


if __name__ == "__main__":
    unittest.main()
