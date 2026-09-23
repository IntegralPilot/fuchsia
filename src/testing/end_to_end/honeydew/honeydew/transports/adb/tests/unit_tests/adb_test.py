# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for adb.py."""

import asyncio
import os
import sys
import unittest
from importlib import resources
from typing import Any
from unittest import mock

from honeydew.transports.adb import adb
from honeydew.transports.adb import errors as adb_errors

_DEVICE_NAME = "fuchsia-mock-device"
_SERIAL_NUMBER = "12345678"


# pylint: disable=protected-access
class AdbTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for ADB transport."""

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()

        self._is_supported_patcher = mock.patch.object(
            adb.Adb,
            "is_supported",
            new_callable=mock.AsyncMock,
            return_value=True,
        )
        self._is_supported_patcher.start()

        self._env_patcher = mock.patch.dict(
            os.environ, {"HONEYDEW_ADB_OVERRIDE": "/custom/adb"}
        )
        self._env_patcher.start()

        self.adb_obj = adb.Adb(
            device_name=_DEVICE_NAME,
            serial_number=_SERIAL_NUMBER,
        )

    async def asyncTearDown(self) -> None:
        self._env_patcher.stop()
        self._is_supported_patcher.stop()
        await super().asyncTearDown()

    @mock.patch("glob.glob", autospec=True)
    def test_check_adb_sysfs_success(self, mock_glob: mock.Mock) -> None:
        """Test _check_adb_sysfs success path."""
        mock_glob.side_effect = lambda pattern: {
            "/sys/bus/usb/devices/*": ["/sys/bus/usb/devices/1-1"],
            "/sys/bus/usb/devices/1-1/*:*.*/bInterfaceClass": [
                "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceClass"
            ],
        }.get(pattern, [])

        mock_files = {
            "/sys/bus/usb/devices/1-1/idVendor": "18d1\n",
            "/sys/bus/usb/devices/1-1/serial": f"{_SERIAL_NUMBER}\n",
            "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceClass": "ff\n",
            "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceSubClass": "42\n",
            "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceProtocol": "01\n",
        }

        def mock_open_file(
            path: str, mode: str = "r", *args: Any, **kwargs: Any
        ) -> mock.MagicMock:
            if path in mock_files:
                return mock.mock_open(read_data=mock_files[path])()
            raise OSError(f"File not found: {path}")

        with mock.patch("os.path.exists", return_value=True), mock.patch(
            "builtins.open", mock_open_file
        ):
            res = adb._check_adb_sysfs(_SERIAL_NUMBER)
            self.assertTrue(res)

    @mock.patch("glob.glob", autospec=True)
    def test_check_adb_sysfs_without_serial_success(
        self, mock_glob: mock.Mock
    ) -> None:
        """Test _check_adb_sysfs success path when target_serial is None."""
        mock_glob.side_effect = lambda pattern: {
            "/sys/bus/usb/devices/*": ["/sys/bus/usb/devices/1-1"],
            "/sys/bus/usb/devices/1-1/*:*.*/bInterfaceClass": [
                "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceClass"
            ],
        }.get(pattern, [])

        mock_files = {
            "/sys/bus/usb/devices/1-1/idVendor": "18d1\n",
            "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceClass": "ff\n",
            "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceSubClass": "42\n",
            "/sys/bus/usb/devices/1-1/1-1:1.0/bInterfaceProtocol": "01\n",
        }

        def mock_open_file(
            path: str, mode: str = "r", *args: Any, **kwargs: Any
        ) -> mock.MagicMock:
            if path in mock_files:
                return mock.mock_open(read_data=mock_files[path])()
            raise OSError(f"File not found: {path}")

        with mock.patch("os.path.exists", return_value=True), mock.patch(
            "builtins.open", mock_open_file
        ):
            res = adb._check_adb_sysfs(None)
            self.assertTrue(res)

    async def test_resolve_serial_number_callable(self) -> None:
        """Test _resolve_serial_number with async provider callback."""

        async def mock_provider() -> str:
            return _SERIAL_NUMBER

        obj = adb.Adb(device_name=_DEVICE_NAME, serial_number=mock_provider)
        res = await obj._resolve_serial_number()
        self.assertEqual(res, _SERIAL_NUMBER)

    @mock.patch.object(resources, "files", autospec=True)
    def test_get_adb_binary_with_env_var_success(
        self, mock_files: mock.Mock
    ) -> None:
        """Test _get_adb_binary when environment variable override is provided."""
        with mock.patch.dict(
            os.environ, {"HONEYDEW_ADB_OVERRIDE": "/env/path/adb"}, clear=True
        ):
            bin_name = adb._get_adb_binary()
            self.assertEqual(bin_name, "/env/path/adb")
            mock_files.assert_not_called()

    @mock.patch.object(resources, "files", autospec=True)
    @mock.patch.object(resources, "as_file", autospec=True)
    @mock.patch("atexit.register", autospec=True)
    @mock.patch("shutil.copy2", autospec=True)
    @mock.patch("tempfile.NamedTemporaryFile", autospec=True)
    def test_get_adb_binary_with_resource_success(
        self,
        mock_tmp_file: mock.Mock,
        mock_copy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Test _get_adb_binary when adb data resource exists."""
        with (
            mock.patch.dict(sys.modules, {"honeydew.data": mock.Mock()}),
            mock.patch.dict(os.environ, {}, clear=True),
        ):
            mock_fd = mock.Mock()
            mock_fd.name = "tmpadb"
            mock_tmp_file.return_value = mock_fd
            bin_name = adb._get_adb_binary()
            self.assertEqual(bin_name, "tmpadb")
            mock_fd.close.assert_called_once()
            mock_copy.assert_called_with(mock.ANY, bin_name)

    @mock.patch("shutil.which", return_value="/which/adb", autospec=True)
    @mock.patch.object(
        resources, "as_file", side_effect=FileNotFoundError, autospec=True
    )
    def test_get_adb_binary_fallback_to_path_success(
        self, mock_as_file: mock.Mock, mock_which: mock.Mock
    ) -> None:
        """Test _get_adb_binary falls back to PATH when resource is not available."""
        with mock.patch.dict(os.environ, {}, clear=True):
            bin_name = adb._get_adb_binary()
            self.assertEqual(bin_name, "/which/adb")
            mock_which.assert_called_once_with("adb")

    @mock.patch("shutil.which", return_value=None, autospec=True)
    @mock.patch.object(
        resources, "as_file", side_effect=FileNotFoundError, autospec=True
    )
    def test_get_adb_binary_not_found_fail(
        self, mock_as_file: mock.Mock, mock_which: mock.Mock
    ) -> None:
        """Test _get_adb_binary raises InitializationError when binary is not found anywhere."""
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(adb_errors.InitializationError):
                adb._get_adb_binary()
            mock_which.assert_called_once_with("adb")

    @mock.patch("shutil.which", return_value="/usr/bin/adb", autospec=True)
    @mock.patch.object(
        resources, "as_file", side_effect=FileNotFoundError, autospec=True
    )
    async def test_make_ready_with_serial(
        self, mock_as_file: mock.Mock, mock_which: mock.Mock
    ) -> None:
        """Test make_ready when serial number is provided."""
        with mock.patch.dict(os.environ, {}, clear=True):
            obj = adb.Adb(
                device_name=_DEVICE_NAME,
                serial_number=_SERIAL_NUMBER,
            )
            await obj.make_ready()
            self.assertTrue(obj._ready)
            self.assertEqual(obj._adb_binary, "/usr/bin/adb")
            self.assertEqual(obj._serial_number, _SERIAL_NUMBER)
            mock_which.assert_called_once_with("adb")

    async def test_make_ready_without_serial(self) -> None:
        """Test make_ready when serial number is not provided."""
        obj = adb.Adb(
            device_name=_DEVICE_NAME,
            serial_number=None,
        )
        with self.assertRaises(adb_errors.InitializationError):
            await obj.make_ready()

    @mock.patch("shutil.which", return_value=None, autospec=True)
    @mock.patch.object(
        resources, "as_file", side_effect=FileNotFoundError, autospec=True
    )
    async def test_make_ready_binary_not_found(
        self, mock_as_file: mock.Mock, mock_which: mock.Mock
    ) -> None:
        """Test make_ready when adb binary is not found anywhere."""
        with mock.patch.dict(os.environ, {}, clear=True):
            obj = adb.Adb(
                device_name=_DEVICE_NAME,
                serial_number=_SERIAL_NUMBER,
            )
            with self.assertRaises(adb_errors.InitializationError):
                await obj.make_ready()

    @mock.patch("asyncio.create_subprocess_exec", autospec=True)
    async def test_run_success(self, mock_create_proc: mock.Mock) -> None:
        """Test run success path."""
        await self.adb_obj.make_ready()

        mock_proc = mock.AsyncMock()
        mock_proc.returncode = 0
        mock_proc.communicate.return_value = (b"output_text\n", b"")
        mock_create_proc.return_value = mock_proc

        output = await self.adb_obj.run(["shell", "echo", "hello"])

        self.assertEqual(output, "output_text")
        mock_create_proc.assert_called_once_with(
            "/custom/adb",
            "-s",
            _SERIAL_NUMBER,
            "shell",
            "echo",
            "hello",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )

    @mock.patch("asyncio.create_subprocess_exec", autospec=True)
    async def test_run_failure(self, mock_create_proc: mock.Mock) -> None:
        """Test run failure path (non-zero exit code)."""
        await self.adb_obj.make_ready()

        mock_proc = mock.AsyncMock()
        mock_proc.returncode = 1
        mock_proc.communicate.return_value = (b"error_text\n", b"")
        mock_create_proc.return_value = mock_proc

        with self.assertRaises(adb_errors.AdbCommandError) as context:
            await self.adb_obj.run(["shell", "bad_command"])

        self.assertIn("error_text", str(context.exception))

    @mock.patch("asyncio.create_subprocess_exec", autospec=True)
    async def test_run_timeout(self, mock_create_proc: mock.Mock) -> None:
        """Test run timeout path."""
        await self.adb_obj.make_ready()

        mock_proc = mock.AsyncMock()
        killed = False

        def mock_kill() -> None:
            nonlocal killed
            killed = True

        mock_proc.kill = mock.Mock(side_effect=mock_kill)

        async def mock_communicate() -> tuple[bytes, bytes]:
            if killed:
                return b"", b""
            await asyncio.sleep(10)
            return b"", b""

        mock_proc.communicate.side_effect = mock_communicate
        mock_create_proc.return_value = mock_proc

        with self.assertRaises(adb_errors.AdbTimeoutError):
            await self.adb_obj.run(["shell", "long_running"], timeout=0.1)

    @mock.patch("asyncio.create_subprocess_exec", autospec=True)
    @mock.patch("asyncio.sleep", new_callable=mock.Mock)
    async def test_run_retry_success(
        self, mock_sleep: mock.Mock, mock_create_proc: mock.Mock
    ) -> None:
        """Test run retries on connection error and succeeds."""

        async def fake_sleep(seconds: float) -> None:
            if seconds >= 10.0:
                await asyncio.get_running_loop().create_future()

        mock_sleep.side_effect = fake_sleep
        await self.adb_obj.make_ready()

        # Set a mocked ADB server
        mock_server = mock.Mock()
        self.adb_obj._adb_server = mock_server

        # First attempt: fail with "device offline"
        mock_proc_fail = mock.AsyncMock()
        mock_proc_fail.returncode = 1
        mock_proc_fail.communicate.return_value = (
            b"error: device offline\n",
            b"",
        )

        # Second attempt: succeed
        mock_proc_success = mock.AsyncMock()
        mock_proc_success.returncode = 0
        mock_proc_success.communicate.return_value = (b"success_output\n", b"")

        mock_create_proc.side_effect = [mock_proc_fail, mock_proc_success]

        output = await self.adb_obj.run(["shell", "some_cmd"])

        self.assertEqual(output, "success_output")
        self.assertEqual(mock_create_proc.call_count, 2)
        mock_server.restart.assert_called_once()
        mock_sleep.assert_any_call(5)

    @mock.patch("asyncio.create_subprocess_exec", autospec=True)
    @mock.patch("asyncio.sleep", new_callable=mock.Mock)
    async def test_run_retry_fail_max_attempts(
        self, mock_sleep: mock.Mock, mock_create_proc: mock.Mock
    ) -> None:
        """Test run retries on connection error but fails after max attempts."""

        async def fake_sleep(seconds: float) -> None:
            if seconds >= 10.0:
                await asyncio.get_running_loop().create_future()

        mock_sleep.side_effect = fake_sleep
        await self.adb_obj.make_ready()

        # Set a mocked ADB server
        mock_server = mock.Mock()
        self.adb_obj._adb_server = mock_server

        # All attempts fail with "device offline"
        def create_fail_proc() -> mock.AsyncMock:
            mock_proc = mock.AsyncMock()
            mock_proc.returncode = 1
            mock_proc.communicate.return_value = (
                b"error: device offline\n",
                b"",
            )
            return mock_proc

        mock_create_proc.side_effect = [
            create_fail_proc(),
            create_fail_proc(),
            create_fail_proc(),
        ]

        with self.assertRaises(adb_errors.AdbCommandError):
            await self.adb_obj.run(["shell", "some_cmd"])

        self.assertEqual(mock_create_proc.call_count, 3)
        self.assertEqual(
            mock_server.restart.call_count, 2
        )  # Restarted on 1st and 2nd failure
        retry_calls = [
            c for c in mock_sleep.call_args_list if c == mock.call(5)
        ]
        self.assertEqual(len(retry_calls), 2)


if __name__ == "__main__":
    unittest.main()
