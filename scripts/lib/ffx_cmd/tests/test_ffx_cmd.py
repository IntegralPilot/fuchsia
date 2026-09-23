# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import pathlib
import unittest

import ffx_cmd
import fx_cmd


class TestFfxCmd(unittest.IsolatedAsyncioTestCase):
    def test_command_line(self) -> None:
        """command lines respect output format flag"""

        inner = fx_cmd.FxCmd(build_directory=pathlib.Path("/fuchsia"))
        actual = ffx_cmd.FfxCmd(inner=inner).command_line("foo")
        self.assertEqual(actual, ["ffx", "foo"])

        actual = ffx_cmd.FfxCmd(
            inner=inner, output_format=ffx_cmd.FfxOutputFormat.JSON
        ).command_line("foo")
        self.assertEqual(actual, ["ffx", "--machine", "json", "foo"])

        actual = ffx_cmd.FfxCmd(
            inner=inner, output_format=ffx_cmd.FfxOutputFormat.PRETTY_JSON
        ).command_line("foo")
        self.assertEqual(actual, ["ffx", "--machine", "json-pretty", "foo"])

    async def test_try_run(self) -> None:
        version = await ffx_cmd.version(
            inner=ffx_cmd.FfxCmd.create_test_inner("host-tools/ffx")
        ).sync()
        self.assertGreater(version.api_level, 0)

    async def test_test_executor_args_without_ffx(self) -> None:
        """TestExecutor preserves all args when 'ffx' is not in the argument list."""
        import tempfile
        import unittest.mock as mock

        with tempfile.NamedTemporaryFile() as tf:
            test_inner = ffx_cmd.FfxCmd.create_test_inner(tf.name)
            with mock.patch(
                "async_utils.command.AsyncCommand.create"
            ) as mock_create:
                await test_inner.start("target", "list")
                mock_create.assert_called_once_with(tf.name, "target", "list")
