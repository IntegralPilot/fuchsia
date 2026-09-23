# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for setup_cog_workspace."""

import logging
import unittest
from unittest.mock import MagicMock, patch

import preflight
import setup_cog_workspace


class TestSetupCogWorkspace(unittest.TestCase):
    """Tests for setup_cog_workspace."""

    def test_main_failed_preflight(self) -> None:
        """Test that main returns 1 when preflight check fails."""
        with patch.object(
            setup_cog_workspace,
            "_parse_args",
            return_value=MagicMock(
                log_level=logging.INFO,
                snapshot=False,
                enable_status_updates=False,
                color=True,
            ),
        ), patch.object(preflight, "check_all", return_value=False), patch(
            "setup_cog_workspace.logger.log_error"
        ) as mock_log_error:
            result = setup_cog_workspace.main()
            self.assertEqual(result, 1)
            mock_log_error.assert_not_called()

    def test_main_success_needs_init(self) -> None:
        """Test that main returns 0 on success and calls init_cartfs."""
        mock_ws = MagicMock()
        mock_ws.has_cartfs_dir = False
        mock_ws.is_checkout_uptodate.return_value = False

        mock_lock = MagicMock()
        mock_ws.lock.return_value = mock_lock

        with patch.object(
            setup_cog_workspace,
            "_parse_args",
            return_value=MagicMock(
                log_level=logging.INFO,
                snapshot=False,
                enable_status_updates=False,
                color=True,
            ),
        ), patch.object(preflight, "check_all", return_value=True), patch(
            "setup_cog_workspace.workspace.Workspace", return_value=mock_ws
        ):
            result = setup_cog_workspace.main()
            self.assertEqual(result, 0)
            mock_ws.init_cartfs_workspace.assert_called_once_with(False)
            mock_ws.checkout_cartfs_to_cog_revisions.assert_called_once()

    def test_main_keyboard_interrupt(self) -> None:
        """Test that main returns 130 on KeyboardInterrupt."""
        with patch.object(
            setup_cog_workspace,
            "_parse_args",
            return_value=MagicMock(
                log_level=logging.INFO,
                snapshot=False,
                enable_status_updates=False,
                color=True,
            ),
        ), patch.object(
            preflight, "check_all", side_effect=KeyboardInterrupt
        ), patch(
            "setup_cog_workspace.logger.log_error"
        ) as mock_log_error:
            result = setup_cog_workspace.main()
            self.assertEqual(result, 130)
            mock_log_error.assert_called_once_with(
                "Workspace setup cancelled by user (KeyboardInterrupt)."
            )


if __name__ == "__main__":
    unittest.main()
