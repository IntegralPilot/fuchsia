# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.starnix.starnix_using_ffx.py."""

import unittest
from unittest import mock

from honeydew import errors
from honeydew.affordances.starnix import errors as starnix_errors
from honeydew.affordances.starnix import starnix_using_ffx
from honeydew.transports.ffx import ffx as ffx_transport


# pylint: disable=protected-access
class StarnixUsingStarnixTests(unittest.TestCase):
    """Unit tests for honeydew.affordances.starnix.starnix_using_ffx.py."""

    def setUp(self) -> None:
        super().setUp()

        self.mock_ffx = mock.MagicMock(spec=ffx_transport.FFX)
        self.mock_ffx.run.return_value = (
            starnix_using_ffx._STARNIX_KERNEL_PREFIX
        )

        self.starnix_obj = starnix_using_ffx.StarnixUsingFfx(
            ffx=self.mock_ffx,
            device_name="device",
        )

        # called in verify_supported()
        self.mock_ffx.run.assert_called_once_with(["component", "list"])

    @mock.patch(
        "os.read",
        return_value=starnix_using_ffx._RegExPatterns.STARNIX_CMD_SUCCESS.pattern.encode(),
        autospec=True,
    )
    @mock.patch(
        "pty.openpty",
        return_value=(1, 1),
        autospec=True,
    )
    async def test_run_console_shell_cmd(
        self, mock_openpty: mock.Mock, mock_os_read: mock.Mock
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.run_console_shell_cmd()"""
        await self.starnix_obj.run_console_shell_cmd(cmd=["something"])
        mock_openpty.assert_called_once()
        mock_os_read.assert_called_once()

    @mock.patch(
        "os.read",
        return_value=starnix_using_ffx._RegExPatterns.STARNIX_NOT_SUPPORTED.pattern.encode(),
        autospec=True,
    )
    @mock.patch(
        "pty.openpty",
        return_value=(1, 1),
        autospec=True,
    )
    async def test_run_console_shell_cmd_exception(
        self, mock_openpty: mock.Mock, mock_os_read: mock.Mock
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.run_console_shell_cmd()
        raising NotSupportedError"""
        with self.assertRaises(errors.NotSupportedError):
            await self.starnix_obj.run_console_shell_cmd(cmd=["something"])
        mock_openpty.assert_called_once()
        mock_os_read.assert_called_once()

    @mock.patch(
        "os.read",
        return_value="something".encode(),
        autospec=True,
    )
    @mock.patch(
        "pty.openpty",
        return_value=(1, 1),
        autospec=True,
    )
    async def test_run_console_shell_cmd_ffx_error(
        self, mock_openpty: mock.Mock, mock_os_read: mock.Mock
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.run_console_shell_cmd()
        raising StarnixError"""
        with self.assertRaises(starnix_errors.StarnixError):
            await self.starnix_obj.run_console_shell_cmd(cmd=["something"])
        mock_openpty.assert_called_once()
        mock_os_read.assert_called_once()
