# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for linux_virtual_usb_hub.py."""

import platform
import unittest
from typing import Any
from unittest import mock

from honeydew import errors
from honeydew.auxiliary_devices.usb_power_hub import (
    linux_virtual_usb_hub,
    usb_power_hub,
)
from honeydew.utils import host_shell


# pylint: disable=protected-access
# pylint: disable=unused-argument
class LinuxVirtualUsbPowerHubTests(unittest.TestCase):
    """Unit tests for linux_virtual_usb_hub.py."""

    def setUp(self) -> None:
        super().setUp()

        # Default to Linux host for tests, unless overridden
        self.platform_patcher = mock.patch.object(
            platform, "system", return_value="Linux"
        )
        self.platform_patcher.start()

    def tearDown(self) -> None:
        self.platform_patcher.stop()
        super().tearDown()

    @mock.patch("builtins.open", new_callable=mock.mock_open)
    @mock.patch("os.path.exists", return_value=True)
    @mock.patch("glob.glob", return_value=["/sys/bus/usb/devices/1-6"])
    def test_init_discovery_success(
        self,
        mock_glob: mock.Mock,
        mock_exists: mock.Mock,
        mock_open_file: mock.Mock,
    ) -> None:
        """Test instantiation with successful auto-discovery."""

        def mock_open_side_effect(
            filepath: str, *args: Any, **kwargs: Any
        ) -> Any:
            if "idVendor" in filepath:
                return mock.mock_open(read_data="18d1\n")()
            elif "idProduct" in filepath:
                return mock.mock_open(read_data="a02b\n")()
            return mock.mock_open()()

        mock_open_file.side_effect = mock_open_side_effect

        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()
        self.assertEqual(hub._usb_bus_id, "1-6")
        self.assertIsInstance(hub, usb_power_hub.UsbPowerHub)

    def test_init_non_linux_raises_error(self) -> None:
        """Test instantiation on non-Linux host raises error."""
        self.platform_patcher.stop()
        with mock.patch.object(platform, "system", return_value="Darwin"):
            with self.assertRaisesRegex(
                usb_power_hub.UsbPowerHubError,
                "LinuxVirtualUsbPowerHub is only supported on Linux",
            ):
                linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()
        self.platform_patcher.start()

    @mock.patch("glob.glob", return_value=[])
    def test_init_discovery_failure_no_device(
        self, mock_glob: mock.Mock
    ) -> None:
        """Test instantiation fails when no matching device is found."""
        with self.assertRaisesRegex(ValueError, "No USB device with vendor"):
            linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()

    @mock.patch("builtins.open", new_callable=mock.mock_open)
    @mock.patch("os.path.exists", return_value=True)
    @mock.patch(
        "glob.glob",
        return_value=["/sys/bus/usb/devices/1-6", "/sys/bus/usb/devices/1-7"],
    )
    def test_init_discovery_failure_multiple_devices(
        self,
        mock_glob: mock.Mock,
        mock_exists: mock.Mock,
        mock_open_file: mock.Mock,
    ) -> None:
        """Test instantiation fails when multiple matching devices are found."""

        def mock_open_side_effect(
            filepath: str, *args: Any, **kwargs: Any
        ) -> Any:
            if "idVendor" in filepath:
                return mock.mock_open(read_data="18d1\n")()
            elif "idProduct" in filepath:
                return mock.mock_open(read_data="a02b\n")()
            return mock.mock_open()()

        mock_open_file.side_effect = mock_open_side_effect

        with self.assertRaisesRegex(ValueError, "Multiple USB devices"):
            linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()

    @mock.patch.object(
        linux_virtual_usb_hub.LinuxVirtualUsbPowerHub,
        "_find_usb_bus_id",
        return_value="1-6; rm -rf /",
    )
    def test_init_with_invalid_bus_id_raises_error(
        self, mock_find_bus_id: mock.Mock
    ) -> None:
        """Test instantiation with invalid bus ID pattern raises error."""
        with self.assertRaisesRegex(ValueError, "Invalid usb_bus_id format"):
            linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()

    @mock.patch.object(
        linux_virtual_usb_hub.LinuxVirtualUsbPowerHub,
        "_find_usb_bus_id",
        return_value="1-6",
    )
    @mock.patch.object(host_shell, "run", autospec=True)
    def test_power_off_success(
        self, mock_run: mock.Mock, mock_find_bus_id: mock.Mock
    ) -> None:
        """Test power_off success path without sudo."""
        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub(use_sudo=False)
        hub.power_off()
        mock_run.assert_called_once_with(
            cmd=["sh", "-c", "echo 0 > /sys/bus/usb/devices/1-6/authorized"]
        )

    @mock.patch.object(
        linux_virtual_usb_hub.LinuxVirtualUsbPowerHub,
        "_find_usb_bus_id",
        return_value="1-6",
    )
    @mock.patch.object(host_shell, "run", autospec=True)
    def test_power_off_success_with_sudo(
        self, mock_run: mock.Mock, mock_find_bus_id: mock.Mock
    ) -> None:
        """Test power_off success path with sudo."""
        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub(use_sudo=True)
        hub.power_off()
        mock_run.assert_called_once_with(
            cmd=[
                "sudo",
                "sh",
                "-c",
                "echo 0 > /sys/bus/usb/devices/1-6/authorized",
            ]
        )

    @mock.patch.object(
        linux_virtual_usb_hub.LinuxVirtualUsbPowerHub,
        "_find_usb_bus_id",
        return_value="1-6",
    )
    @mock.patch.object(
        host_shell,
        "run",
        side_effect=errors.HostCmdError("error"),
        autospec=True,
    )
    def test_power_off_failure_raises_error(
        self, mock_run: mock.Mock, mock_find_bus_id: mock.Mock
    ) -> None:
        """Test power_off failure raises UsbPowerHubError."""
        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()
        with self.assertRaises(usb_power_hub.UsbPowerHubError):
            hub.power_off()

    @mock.patch.object(
        linux_virtual_usb_hub.LinuxVirtualUsbPowerHub,
        "_find_usb_bus_id",
        return_value="1-6",
    )
    @mock.patch.object(host_shell, "run", autospec=True)
    def test_power_on_success(
        self, mock_run: mock.Mock, mock_find_bus_id: mock.Mock
    ) -> None:
        """Test power_on success path without sudo."""
        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub(use_sudo=False)
        hub.power_on()
        mock_run.assert_called_once_with(
            cmd=["sh", "-c", "echo 1 > /sys/bus/usb/devices/1-6/authorized"]
        )

    @mock.patch.object(
        linux_virtual_usb_hub.LinuxVirtualUsbPowerHub,
        "_find_usb_bus_id",
        return_value="1-6",
    )
    @mock.patch.object(
        host_shell,
        "run",
        side_effect=errors.HostCmdError("error"),
        autospec=True,
    )
    def test_power_on_failure_raises_error(
        self, mock_run: mock.Mock, mock_find_bus_id: mock.Mock
    ) -> None:
        """Test power_on failure raises UsbPowerHubError."""
        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub()
        with self.assertRaises(usb_power_hub.UsbPowerHubError):
            hub.power_on()

    # Tests for discovery with serial number matching
    @mock.patch("builtins.open", new_callable=mock.mock_open)
    @mock.patch("os.path.exists", return_value=True)
    @mock.patch(
        "glob.glob",
        return_value=["/sys/bus/usb/devices/1-6", "/sys/bus/usb/devices/1-7"],
    )
    def test_init_discovery_with_explicit_serial_success(
        self,
        mock_glob: mock.Mock,
        mock_exists: mock.Mock,
        mock_open_file: mock.Mock,
    ) -> None:
        """Test discovery matches serial number passed in constructor."""

        def mock_open_side_effect(
            filepath: str, *args: Any, **kwargs: Any
        ) -> Any:
            if "idVendor" in filepath:
                return mock.mock_open(read_data="18d1\n")()
            elif "idProduct" in filepath:
                return mock.mock_open(read_data="a02b\n")()
            elif "1-6/serial" in filepath:
                return mock.mock_open(read_data="OTHER_SERIAL\n")()
            elif "1-7/serial" in filepath:
                return mock.mock_open(read_data="MY_SERIAL\n")()
            return mock.mock_open()()

        mock_open_file.side_effect = mock_open_side_effect

        hub = linux_virtual_usb_hub.LinuxVirtualUsbPowerHub(
            target_serial="MY_SERIAL"
        )
        self.assertEqual(hub._usb_bus_id, "1-7")

    @mock.patch("builtins.open", new_callable=mock.mock_open)
    @mock.patch("os.path.exists", return_value=True)
    @mock.patch(
        "glob.glob",
        return_value=["/sys/bus/usb/devices/1-6", "/sys/bus/usb/devices/1-7"],
    )
    def test_init_discovery_with_non_matching_serial_raises_error(
        self,
        mock_glob: mock.Mock,
        mock_exists: mock.Mock,
        mock_open_file: mock.Mock,
    ) -> None:
        """Test discovery fails when specified target_serial does not match."""

        def mock_open_side_effect(
            filepath: str, *args: Any, **kwargs: Any
        ) -> Any:
            if "idVendor" in filepath:
                return mock.mock_open(read_data="18d1\n")()
            elif "idProduct" in filepath:
                return mock.mock_open(read_data="a02b\n")()
            elif "1-6/serial" in filepath:
                return mock.mock_open(read_data="OTHER_SERIAL\n")()
            elif "1-7/serial" in filepath:
                return mock.mock_open(read_data="MY_SERIAL\n")()
            return mock.mock_open()()

        mock_open_file.side_effect = mock_open_side_effect

        with self.assertRaisesRegex(ValueError, "No USB device with vendor"):
            linux_virtual_usb_hub.LinuxVirtualUsbPowerHub(
                target_serial="NON_EXISTENT_SERIAL"
            )
