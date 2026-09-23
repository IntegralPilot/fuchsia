# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import unittest
import unittest.mock as mock
from pathlib import Path

import package_server


class TestPackageServer(unittest.IsolatedAsyncioTestCase):
    @mock.patch("package_server.lib.FxCmd")
    async def test_is_running_true(self, mock_fx_cmd: mock.Mock) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        process_mock.return_code = 0
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        # Configure start to be an async mock that returns cmd_mock
        mock_fx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        self.assertTrue(await package_server.is_running())
        mock_fx_cmd.return_value.start.assert_called_with(
            "is-package-server-running"
        )

    @mock.patch("package_server.lib.FxCmd")
    async def test_is_running_false(self, mock_fx_cmd: mock.Mock) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        process_mock.return_code = 1
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        mock_fx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        self.assertFalse(await package_server.is_running())

    @mock.patch("package_server.lib.is_running")
    async def test_wait_for_package_server_success(
        self, mock_is_running: mock.Mock
    ) -> None:
        mock_is_running.return_value = True
        process_mock = mock.Mock()
        process_mock.wait = mock.Mock(return_value=asyncio.Future())

        self.assertTrue(
            await package_server.wait_for_package_server(
                process_mock, timeout=1, interval=0.1
            )
        )

    @mock.patch("package_server.lib.is_running")
    async def test_wait_for_package_server_process_exits(
        self, mock_is_running: mock.Mock
    ) -> None:
        mock_is_running.return_value = False
        process_mock = mock.Mock()
        process_mock.wait = mock.AsyncMock(return_value=None)

        self.assertFalse(
            await package_server.wait_for_package_server(
                process_mock, timeout=1, interval=0.1
            )
        )

    @mock.patch("package_server.lib.is_running")
    async def test_wait_for_package_server_timeout(
        self,
        mock_is_running: mock.Mock,
    ) -> None:
        mock_is_running.return_value = False
        process_mock = mock.Mock()
        process_mock.wait = mock.Mock(return_value=asyncio.Future())

        self.assertFalse(
            await package_server.wait_for_package_server(
                process_mock, timeout=0.01, interval=0.001
            )
        )

    @mock.patch("package_server.lib.build_dir.get_build_directory")
    @mock.patch("package_server.lib.FfxCmd")
    async def test_get_arguments(
        self, mock_ffx_cmd: mock.Mock, mock_get_build_dir: mock.Mock
    ) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        process_mock.stdout = "8084\n"
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        mock_ffx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        mock_get_build_dir.return_value = Path("/some/out/dir")

        args = await package_server.get_arguments()
        self.assertIn("server", args)
        self.assertIn("start", args)
        self.assertIn("[::]:8084", args)
        self.assertNotIn("fx", args)
        self.assertNotIn("ffx", args)
        mock_ffx_cmd.return_value.start.assert_called_with(
            "config",
            "get",
            "repository.server.default_port",
        )

    @mock.patch("package_server.lib.build_dir.get_build_directory")
    @mock.patch("package_server.lib.FfxCmd")
    async def test_get_arguments_with_name(
        self, mock_ffx_cmd: mock.Mock, mock_get_build_dir: mock.Mock
    ) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        process_mock.stdout = "8083\n"
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        mock_ffx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        mock_get_build_dir.return_value = Path("/some/out/dir")

        args = await package_server.get_arguments(name="my-repo")
        self.assertIn("my-repo", args)

    @mock.patch("package_server.lib.build_dir.get_build_directory")
    @mock.patch("package_server.lib.FfxCmd")
    async def test_get_arguments_ffx_config_failure(
        self, mock_ffx_cmd: mock.Mock, mock_get_build_dir: mock.Mock
    ) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        process_mock.stdout = "invalid\n"
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        mock_ffx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        mock_get_build_dir.return_value = Path("/some/out/dir")

        args = await package_server.get_arguments()
        # Should fallback to default port 8083
        self.assertIn("[::]:8083", args)

    @mock.patch("package_server.lib.build_dir.get_build_directory")
    def test_is_package_repository_built(
        self, mock_get_build_dir: mock.Mock
    ) -> None:
        mock_path = mock.MagicMock()
        mock_get_build_dir.return_value = mock_path
        repo_json = mock_path / "amber-files" / "repository" / "9.root.json"

        repo_json.is_file.return_value = True
        self.assertTrue(package_server.is_package_repository_built())

        repo_json.is_file.return_value = False
        self.assertFalse(package_server.is_package_repository_built())

    @mock.patch("package_server.lib.is_package_repository_built")
    @mock.patch("package_server.lib.wait_for_package_server")
    @mock.patch("package_server.lib.get_arguments")
    @mock.patch("package_server.lib.FfxCmd")
    @mock.patch("package_server.lib.FxCmd")
    @mock.patch("asyncio.create_subprocess_exec")
    async def test_start_success(
        self,
        mock_exec: mock.Mock,
        mock_fx_cmd: mock.Mock,
        mock_ffx_cmd: mock.Mock,
        mock_get_arguments: mock.Mock,
        mock_wait: mock.Mock,
        mock_is_built: mock.Mock,
    ) -> None:
        mock_is_built.return_value = True
        mock_wait.return_value = True
        mock_get_arguments.return_value = ("arg1", "arg2")
        mock_ffx_cmd.return_value.command_line.side_effect = (
            lambda *args: ("ffx",) + args
        )
        mock_fx_cmd.return_value.command_line.side_effect = (
            lambda *args: ("fx",) + args
        )

        # Mock for package server process
        server_process_mock = mock.Mock()
        mock_exec.return_value = server_process_mock

        self.assertEqual(await package_server.start(), server_process_mock)
        mock_ffx_cmd.return_value.command_line.assert_called_with(
            "arg1", "arg2"
        )
        mock_fx_cmd.return_value.command_line.assert_called_with(
            "ffx", "arg1", "arg2"
        )
        mock_exec.assert_called_with(
            "fx",
            "ffx",
            "arg1",
            "arg2",
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        mock_wait.assert_called_with(server_process_mock, 30, 0.2)

    @mock.patch("package_server.lib.is_package_repository_built")
    async def test_start_fails_when_repo_not_built(
        self,
        mock_is_built: mock.Mock,
    ) -> None:
        mock_is_built.return_value = False
        with self.assertRaisesRegex(
            package_server.PackageServingException,
            "The package repository is not built!",
        ):
            await package_server.start()

    @mock.patch("package_server.lib.FfxCmd")
    async def test_stop(self, mock_ffx_cmd: mock.Mock) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        mock_ffx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        await package_server.stop(name="my-repo")
        mock_ffx_cmd.return_value.start.assert_called_with(
            "repository",
            "server",
            "stop",
            "my-repo",
        )
        cmd_mock.run_to_completion.assert_called()

    @mock.patch("package_server.lib.FfxCmd")
    async def test_stop_all(self, mock_ffx_cmd: mock.Mock) -> None:
        cmd_mock = mock.Mock()
        process_mock = mock.Mock()
        cmd_mock.run_to_completion = mock.AsyncMock(return_value=process_mock)
        mock_ffx_cmd.return_value.start = mock.AsyncMock(return_value=cmd_mock)

        await package_server.stop()
        mock_ffx_cmd.return_value.start.assert_called_with(
            "repository",
            "server",
            "stop",
        )
        cmd_mock.run_to_completion.assert_called()

    @mock.patch("package_server.lib.is_running")
    async def test_ensure_running_already_running(
        self,
        mock_is_running: mock.Mock,
    ) -> None:
        mock_is_running.return_value = True
        async with package_server.ensure_running():
            pass
        mock_is_running.assert_called()

    @mock.patch("package_server.lib.stop")
    @mock.patch("package_server.lib.start")
    @mock.patch("package_server.lib.is_running")
    async def test_ensure_running_starts_and_stops(
        self,
        mock_is_running: mock.Mock,
        mock_start: mock.Mock,
        mock_stop: mock.Mock,
    ) -> None:
        mock_is_running.return_value = False
        process_mock = mock.Mock()
        process_mock.terminate = mock.Mock()
        process_mock.wait = mock.AsyncMock(return_value=None)
        mock_start.return_value = process_mock

        async with package_server.ensure_running():
            pass

        mock_is_running.assert_called()
        mock_start.assert_called()
        mock_stop.assert_called()

        # Verify start and stop called with same repo name
        self.assertEqual(mock_start.call_args[0][0], mock_stop.call_args[0][0])

        process_mock.terminate.assert_called()
        process_mock.wait.assert_called()

    @mock.patch("package_server.lib.start")
    @mock.patch("package_server.lib.is_running")
    async def test_ensure_running_failure(
        self,
        mock_is_running: mock.Mock,
        mock_start: mock.Mock,
    ) -> None:
        mock_is_running.return_value = False
        mock_start.side_effect = package_server.PackageServingException("fail")

        with self.assertRaises(package_server.PackageServingCLIException):
            async with package_server.ensure_running():
                pass
