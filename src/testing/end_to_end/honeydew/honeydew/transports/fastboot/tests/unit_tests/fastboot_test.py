# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for fastboot.py."""

import os
import sys
import unittest
from collections.abc import Callable
from importlib import resources
from typing import Any
from unittest import mock

from honeydew import affordances_capable, errors
from honeydew.auxiliary_devices.power_switch import (
    power_switch as power_switch_interface,
)
from honeydew.transports.fastboot import errors as fastboot_errors
from honeydew.transports.fastboot import fastboot
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx
from honeydew.transports.serial import errors as serial_errors
from honeydew.transports.serial import serial as serial_interface
from honeydew.utils import host_shell
from parameterized import param, parameterized

_USB_BASED_DEVICE_NAME: str = "fuchsia-d88c-799b-0e3a"
_USB_BASED_FASTBOOT_NODE_ID: str = "0B190YCABZZ2ML"

_TCP_BASED_DEVICE_NAME: str = "fuchsia-54b2-038b-6e90"
_TCP_IP_ADDRESS: str = "fe80::56b2:3ff:fe8b:6e90%enxa0cec8f442ce"
_TCP_BASED_FASTBOOT_NODE_ID: str = f"tcp:{_TCP_IP_ADDRESS}"

_USB_BASED_TARGET_WHEN_IN_FUCHSIA_MODE: dict[str, Any] = {
    "nodename": _USB_BASED_DEVICE_NAME,
    "rcs_state": "Y",
    "serial": _USB_BASED_FASTBOOT_NODE_ID,
    "target_type": "someproduct_latest_eng.someproduct",
    "target_state": "Product",
    "addresses": [
        "fe80::de1d:c975:e647:cf39%zx-d88c799b0e3b",
        "172.16.243.231",
    ],
    "is_default": True,
}

_USB_BASED_TARGET_WHEN_IN_FASTBOOT_MODE: dict[str, Any] = {
    "nodename": _USB_BASED_DEVICE_NAME,
    "rcs_state": "N",
    "serial": _USB_BASED_FASTBOOT_NODE_ID,
    "target_type": "someproduct_latest_eng.someproduct",
    "target_state": "Fastboot",
    "addresses": [],
    "is_default": True,
}

_TCP_BASED_TARGET_WHEN_IN_FUCHSIA_MODE: dict[str, Any] = {
    "nodename": _TCP_BASED_DEVICE_NAME,
    "rcs_state": "Y",
    "serial": "<unknown>",
    "target_type": "core.x64",
    "target_state": "Product",
    "addresses": ["fe80::881b:4248:1002:a7ce%enxa0cec8f442ce"],
    "is_default": True,
}

_TCP_BASED_TARGET_WHEN_IN_FASTBOOT_MODE: dict[str, Any] = {
    "nodename": _TCP_BASED_DEVICE_NAME,
    "rcs_state": "N",
    "serial": "<unknown>",
    "target_type": "core.x64",
    "target_state": "Fastboot",
    "addresses": [_TCP_IP_ADDRESS],
    "is_default": True,
}

_TCP_BASED_TARGET_WHEN_IN_FASTBOOT_MODE_WITH_TWO_IPS: dict[str, Any] = {
    "nodename": _TCP_BASED_DEVICE_NAME,
    "rcs_state": "N",
    "serial": "<unknown>",
    "target_type": "core.x64",
    "target_state": "Fastboot",
    "addresses": ["fe80::881b:4248:1002:a7ce%enxa0cec8f442ce", _TCP_IP_ADDRESS],
    "is_default": True,
}

_INPUT_ARGS: dict[str, Any] = {
    "device_name": _USB_BASED_DEVICE_NAME,
    "fastboot_node_id": _USB_BASED_FASTBOOT_NODE_ID,
    "run_cmd": ["getvar", "hw-revision"],
}

_FASTBOOT_DEVICES: str = f"{_USB_BASED_FASTBOOT_NODE_ID}\t Android Fastboot"

_MOCK_ARGS: dict[str, Any] = {
    "ffx_target_info_when_in_fuchsia_mode": _USB_BASED_TARGET_WHEN_IN_FUCHSIA_MODE,
    "ffx_target_info_when_in_fastboot_mode": _USB_BASED_TARGET_WHEN_IN_FASTBOOT_MODE,
    "fastboot_devices_when_in_fuchsia_mode": "",
    "fastboot_devices_when_in_fastboot_mode": _FASTBOOT_DEVICES,
    "fastboot_getvar_hw_revision": "hw-revision: core.x64-b4\nFinished. Total time: 0.000s",
}

_EXPECTED_VALUES: dict[str, Any] = {
    "fastboot_run_getvar_hw_revision": ["hw-revision: core.x64-b4"],
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = testcase_func.__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_{test_label}"


# pylint: disable=protected-access
class FastbootTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for Fastboot."""

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()

        self.reboot_affordance_obj = mock.AsyncMock(
            spec=affordances_capable.RebootCapableDevice
        )

        self.ffx_obj = mock.MagicMock(spec=ffx.FFX)

        self.fastboot_obj = fastboot.Fastboot(
            device_name=_INPUT_ARGS["device_name"],
            reboot_affordance=self.reboot_affordance_obj,
            ffx_transport=self.ffx_obj,
            fastboot_node_id=_INPUT_ARGS["fastboot_node_id"],
        )
        # Ensure it's ready for most tests
        await self.fastboot_obj.make_ready()

    @mock.patch(
        "importlib.resources.files",
        autospec=True,
    )
    def test_get_fastboot_binary_with_env_var_success(
        self, mock_files: mock.Mock
    ) -> None:
        """Test case for _get_fastboot_binary() when environment
        variable override is provided"""
        with mock.patch.dict(
            os.environ, {"HONEYDEW_FASTBOOT_OVERRIDE": "fastboot"}, clear=True
        ):
            bin_name = fastboot._get_fastboot_binary()
            self.assertEqual(bin_name, "fastboot")
            mock_files.assert_not_called()

    @mock.patch.object(resources, "files", autospec=True)
    @mock.patch.object(resources, "as_file", autospec=True)
    @mock.patch("atexit.register", autospec=True)
    @mock.patch("shutil.copy2", autospec=True)
    @mock.patch("tempfile.NamedTemporaryFile", autospec=True)
    def test_get_fastboot_binary_with_resource_success(
        self,
        mock_tmp_file: mock.Mock,
        mock_copy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Test case for _get_fastboot_binary() when fastboot data exists"""
        sys.modules["honeydew.data"] = mock.Mock()
        with mock.patch.dict(os.environ, {}, clear=True):
            mock_fd = mock.Mock()
            mock_fd.name = "tmpfastboot"
            mock_tmp_file.return_value = mock_fd
            bin_name = fastboot._get_fastboot_binary()
            self.assertEqual(bin_name, "tmpfastboot")
            mock_copy.assert_called_with(mock.ANY, bin_name)

    @mock.patch.object(resources, "as_file", autospec=True)
    def test_get_fastboot_binary_with_resource_fail(
        self, mock_as_file: mock.Mock
    ) -> None:
        """Test case for _get_fastboot_binary() when fastboot data does not
        exist"""
        mock_file = mock.Mock()
        mock_file.stat.side_effect = ImportError
        mock_as_file.return_value.__enter__.return_value = mock_file
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(errors.HoneydewDataResourceError):
                fastboot._get_fastboot_binary()

    async def test_node_id_when_fastboot_node_id_passed(self) -> None:
        """Testcase for Fastboot.node_id when `fastboot_node_id` arg was passed
        during initialization"""
        self.assertEqual(
            await self.fastboot_obj.node_id(), _INPUT_ARGS["fastboot_node_id"]
        )

    @mock.patch.object(
        fastboot.Fastboot,
        "wait_for_fuchsia_mode",
        side_effect=errors.FuchsiaDeviceError("error"),
        autospec=True,
    )
    async def test_boot_to_fastboot_mode_when_not_in_fuchsia_mode(
        self, mock_wait_for_fuchsia_mode: mock.Mock
    ) -> None:
        """Test case for Fastboot.boot_to_fastboot_mode() when device is not in
        fuchsia mode"""
        with self.assertRaises(errors.FuchsiaStateError):
            await self.fastboot_obj.boot_to_fastboot_mode(use_serial=False)

        mock_wait_for_fuchsia_mode.assert_called()

    @mock.patch.object(
        fastboot.Fastboot,
        "wait_for_fastboot_mode",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "_boot_to_fastboot_mode_using_ffx",
        autospec=True,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "wait_for_fuchsia_mode",
        autospec=True,
    )
    async def test_boot_to_fastboot_mode_when_in_fuchsia_mode_with_out_serial(
        self,
        mock_wait_for_fuchsia_mode: mock.Mock,
        mock_boot_to_fastboot_mode_using_ffx: mock.Mock,
        mock_wait_for_fastboot_mode: mock.AsyncMock,
    ) -> None:
        """Test case for Fastboot.boot_to_fastboot_mode() when device is in fuchsia mode with
        use_serial=False"""
        await self.fastboot_obj.boot_to_fastboot_mode(use_serial=False)

        mock_wait_for_fuchsia_mode.assert_called()
        mock_boot_to_fastboot_mode_using_ffx.assert_called()
        mock_wait_for_fastboot_mode.assert_called()

    @mock.patch.object(
        fastboot.Fastboot,
        "wait_for_fastboot_mode",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "_boot_to_fastboot_mode_using_serial",
        autospec=True,
    )
    async def test_boot_to_fastboot_mode_with_serial(
        self,
        mock_boot_to_fastboot_mode_using_serial: mock.Mock,
        mock_wait_for_fastboot_mode: mock.AsyncMock,
    ) -> None:
        """Test case for Fastboot.boot_to_fastboot_mode() with use_serial set to True"""
        await self.fastboot_obj.boot_to_fastboot_mode(use_serial=True)

        mock_boot_to_fastboot_mode_using_serial.assert_called()
        mock_wait_for_fastboot_mode.assert_called()

    @mock.patch.object(
        fastboot.Fastboot,
        "_boot_to_fastboot_mode_using_serial",
        side_effect=serial_errors.SerialError("error"),
        autospec=True,
    )
    async def test_boot_to_fastboot_mode_exception(
        self,
        mock_boot_to_fastboot_mode_using_serial: mock.Mock,
    ) -> None:
        """Test case for Fastboot.boot_to_fastboot_mode() raising an
        exception"""
        with self.assertRaises(errors.FuchsiaDeviceError):
            await self.fastboot_obj.boot_to_fastboot_mode(use_serial=True)

        mock_boot_to_fastboot_mode_using_serial.assert_called()

    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=False,
    )
    async def test_boot_to_fuchsia_mode_when_not_in_fastboot_mode(
        self, mock_is_in_fastboot_mode: mock.AsyncMock
    ) -> None:
        """Test case for Fastboot.boot_to_fuchsia_mode() when device is not in
        fastboot mode"""
        with self.assertRaises(errors.FuchsiaStateError):
            await self.fastboot_obj.boot_to_fuchsia_mode()

        mock_is_in_fastboot_mode.assert_called()

    @mock.patch.object(
        fastboot.Fastboot, "wait_for_fuchsia_mode", autospec=True
    )
    @mock.patch.object(fastboot.Fastboot, "run", new_callable=mock.AsyncMock)
    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=True,
    )
    async def test_boot_to_fuchsia_mode_when_in_fastboot_mode(
        self,
        mock_is_in_fastboot_mode: mock.AsyncMock,
        mock_fastboot_run: mock.AsyncMock,
        mock_wait_for_fuchsia_mode: mock.Mock,
    ) -> None:
        """Test case for Fastboot.boot_to_fuchsia_mode() when device is in
        fastboot mode"""
        await self.fastboot_obj.boot_to_fuchsia_mode()

        mock_is_in_fastboot_mode.assert_called()
        self.ffx_obj.notify_intentional_disconnect.assert_called_once()
        mock_fastboot_run.assert_called()
        mock_wait_for_fuchsia_mode.assert_called()

    @mock.patch.object(
        fastboot.Fastboot,
        "run",
        new_callable=mock.AsyncMock,
        side_effect=fastboot_errors.FastbootCommandError("error"),
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=True,
    )
    async def test_boot_to_fuchsia_mode_failed(
        self,
        mock_is_in_fastboot_mode: mock.AsyncMock,
        mock_fastboot_run: mock.AsyncMock,
    ) -> None:
        """Test case for Fastboot.boot_to_fuchsia_mode() raising an exception"""
        with self.assertRaises(errors.FuchsiaDeviceError):
            await self.fastboot_obj.boot_to_fuchsia_mode()

        mock_is_in_fastboot_mode.assert_called()
        mock_fastboot_run.assert_called()

    @parameterized.expand(
        [
            (
                {
                    "label": "when_device_is_in_fuchsia_mode",
                    "fastboot_devices": _MOCK_ARGS[
                        "fastboot_devices_when_in_fuchsia_mode"
                    ],
                    "expected": False,
                },
            ),
            (
                {
                    "label": "when_device_is_in_fastboot_mode",
                    "fastboot_devices": _MOCK_ARGS[
                        "fastboot_devices_when_in_fastboot_mode"
                    ],
                    "expected": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        host_shell,
        "run",
        autospec=True,
    )
    async def test_is_in_fastboot_mode(
        self,
        parameterized_dict: dict[str, Any],
        mock_host_shell_run: mock.Mock,
    ) -> None:
        """Test case for Fastboot.is_in_fastboot_mode()"""
        mock_host_shell_run.return_value = parameterized_dict[
            "fastboot_devices"
        ]

        self.assertEqual(
            await self.fastboot_obj.is_in_fastboot_mode(),
            parameterized_dict["expected"],
        )

        mock_host_shell_run.assert_called()

    @mock.patch.object(
        host_shell,
        "run",
        side_effect=errors.HostCmdError("error"),
        autospec=True,
    )
    async def test_is_in_fastboot_mode_fail(
        self,
        mock_host_shell_run: mock.Mock,
    ) -> None:
        """Test case for Fastboot.is_in_fastboot_mode() raising exception."""
        with self.assertRaisesRegex(
            fastboot_errors.FastbootCommandError, "Failed to check"
        ):
            await self.fastboot_obj.is_in_fastboot_mode()

        mock_host_shell_run.assert_called()

    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=False,
    )
    async def test_run_when_not_in_fastboot_mode(
        self, mock_is_in_fastboot_mode: mock.AsyncMock
    ) -> None:
        """Test case for Fastboot.run() when device is not in fastboot mode."""
        with self.assertRaises(errors.FuchsiaStateError):
            await self.fastboot_obj.run(cmd=_INPUT_ARGS["run_cmd"])
        mock_is_in_fastboot_mode.assert_called()

    @mock.patch.object(
        host_shell,
        "run",
        return_value=_MOCK_ARGS["fastboot_getvar_hw_revision"],
        autospec=True,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=True,
    )
    async def test_run_when_in_fastboot_mode_success(
        self,
        mock_is_in_fastboot_mode: mock.AsyncMock,
        mock_host_shell_run: mock.Mock,
    ) -> None:
        """Test case for Fastboot.run() when device is in fastboot mode and
        returns success."""
        self.assertEqual(
            await self.fastboot_obj.run(cmd=_INPUT_ARGS["run_cmd"]),
            _EXPECTED_VALUES["fastboot_run_getvar_hw_revision"],
        )
        mock_is_in_fastboot_mode.assert_called()
        mock_host_shell_run.assert_called()

    @mock.patch.object(
        host_shell,
        "run",
        side_effect=errors.HostCmdError("error"),
        autospec=True,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=True,
    )
    async def test_run_when_in_fastboot_mode_exception(
        self,
        mock_is_in_fastboot_mode: mock.AsyncMock,
        mock_host_shell_run: mock.Mock,
    ) -> None:
        """Test case for Fastboot.run() when device is in fastboot mode and
        returns in exceptions."""

        with self.assertRaises(fastboot_errors.FastbootCommandError):
            await self.fastboot_obj.run(cmd=_INPUT_ARGS["run_cmd"])

        mock_is_in_fastboot_mode.assert_called()
        mock_host_shell_run.assert_called()

    async def test_get_fastboot_node_without_fastboot_node_id_arg_usb_based(
        self,
    ) -> None:
        """Test case for Fastboot._get_fastboot_node() when called without
        fastboot_node_id arg for a USB based fastboot device."""
        self.ffx_obj.get_target_info_from_target_list.return_value = (
            _USB_BASED_TARGET_WHEN_IN_FUCHSIA_MODE
        )
        # Reset node id to force retrieval
        self.fastboot_obj._fastboot_node_id = None
        await self.fastboot_obj._get_fastboot_node()
        self.assertEqual(
            self.fastboot_obj._fastboot_node_id, _USB_BASED_FASTBOOT_NODE_ID
        )

    @mock.patch.object(
        fastboot.Fastboot,
        "boot_to_fuchsia_mode",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "_wait_for_valid_tcp_address",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fastboot.Fastboot,
        "boot_to_fastboot_mode",
        new_callable=mock.AsyncMock,
    )
    async def test_get_fastboot_node_without_fastboot_node_id_arg_tcp_based(
        self,
        mock_boot_to_fastboot_mode: mock.AsyncMock,
        mock_wait_for_valid_tcp_address: mock.AsyncMock,
        mock_boot_to_fuchsia_mode: mock.AsyncMock,
    ) -> None:
        """Test case for Fastboot._get_fastboot_node() when called without
        fastboot_node_id arg for a TCP based fastboot device."""
        self.ffx_obj.get_target_info_from_target_list.side_effect = [
            _TCP_BASED_TARGET_WHEN_IN_FUCHSIA_MODE,
            _TCP_BASED_TARGET_WHEN_IN_FASTBOOT_MODE,
        ]

        # Reset node id to force retrieval
        self.fastboot_obj._fastboot_node_id = None
        await self.fastboot_obj._get_fastboot_node()
        self.assertEqual(
            self.fastboot_obj._fastboot_node_id, _TCP_BASED_FASTBOOT_NODE_ID
        )

        self.assertEqual(
            self.ffx_obj.get_target_info_from_target_list.call_count, 2
        )
        mock_boot_to_fastboot_mode.assert_called()
        mock_wait_for_valid_tcp_address.assert_called()
        mock_boot_to_fuchsia_mode.assert_called()

    async def test_get_fastboot_node_without_fastboot_node_id_arg_exception(
        self,
    ) -> None:
        """Test case for Fastboot._get_fastboot_node() when called without
        fastboot_node_id arg results in an exception."""
        self.ffx_obj.get_target_info_from_target_list.side_effect = (
            ffx_errors.FfxCommandError("error")
        )
        # Reset node id to force retrieval
        self.fastboot_obj._fastboot_node_id = None
        with self.assertRaises(errors.FuchsiaDeviceError):
            await self.fastboot_obj._get_fastboot_node()

    @parameterized.expand(
        [
            (
                {
                    "label": "single_ip_address",
                    "get_target_info": _TCP_BASED_TARGET_WHEN_IN_FASTBOOT_MODE,
                    "expected": True,
                },
            ),
            (
                {
                    "label": "multiple_ip_address",
                    "get_target_info": _TCP_BASED_TARGET_WHEN_IN_FASTBOOT_MODE_WITH_TWO_IPS,
                    "expected": False,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    def test_is_a_single_ip_address(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Test case for Fastboot._is_a_single_ip_address()"""
        self.ffx_obj.get_target_info_from_target_list.return_value = (
            parameterized_dict["get_target_info"]
        )
        self.assertEqual(
            self.fastboot_obj._is_a_single_ip_address(),
            parameterized_dict["expected"],
        )

    @mock.patch.object(
        fastboot.Fastboot,
        "is_in_fastboot_mode",
        new_callable=mock.AsyncMock,
        return_value=True,
    )
    async def test_wait_for_fastboot_mode_success(
        self, mock_is_in_fastboot_mode: mock.AsyncMock
    ) -> None:
        """Test case for Fastboot.wait_for_fastboot_mode() success case."""
        await self.fastboot_obj.wait_for_fastboot_mode()
        mock_is_in_fastboot_mode.assert_called()

    async def test_wait_for_fuchsia_mode_success(self) -> None:
        """Test case for Fastboot.wait_for_fuchsia_mode() success case."""
        await self.fastboot_obj.wait_for_fuchsia_mode()

    @mock.patch.object(
        fastboot.Fastboot,
        "_is_a_single_ip_address",
        return_value=True,
    )
    async def test_wait_for_valid_tcp_address_success(
        self, mock_is_a_single_ip_address: mock.Mock
    ) -> None:
        """Test case for Fastboot._wait_for_valid_tcp_address() success case."""
        await self.fastboot_obj._wait_for_valid_tcp_address()
        mock_is_a_single_ip_address.assert_called()

    @mock.patch("asyncio.sleep", new_callable=mock.AsyncMock)
    @mock.patch(
        "time.time",
        side_effect=[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        autospec=True,
    )
    async def test_boot_to_fastboot_mode_using_serial(
        self, mock_time: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Test case for Fastboot._boot_to_fastboot_mode_using_serial()"""
        serial_transport = mock.MagicMock(spec=serial_interface.Serial)
        power_switch = mock.MagicMock(spec=power_switch_interface.PowerSwitch)

        await self.fastboot_obj._boot_to_fastboot_mode_using_serial(
            serial_transport=serial_transport, power_switch=power_switch
        )

        self.ffx_obj.notify_intentional_disconnect.assert_called_once()
        mock_time.assert_called()
        mock_sleep.assert_called()

    async def test_boot_to_fastboot_mode_using_serial_error(self) -> None:
        """Test case for Fastboot._boot_to_fastboot_mode_using_serial() raising exceptions"""
        with self.assertRaisesRegex(
            ValueError,
            "'power_switch' and 'serial_transport' args need to be provided",
        ):
            await self.fastboot_obj._boot_to_fastboot_mode_using_serial(
                serial_transport=None,
                power_switch=mock.MagicMock(
                    spec=power_switch_interface.PowerSwitch
                ),
            )

            await self.fastboot_obj._boot_to_fastboot_mode_using_serial(
                serial_transport=mock.MagicMock(spec=serial_interface.Serial),
                power_switch=None,
            )

    def test_boot_to_fastboot_mode_using_ffx(self) -> None:
        """Test case for Fastboot.boot_to_fastboot_mode_using_ffx() when device is not in
        fuchsia mode"""
        self.ffx_obj.run.side_effect = ffx_errors.FfxCommandError("error")

        self.fastboot_obj._boot_to_fastboot_mode_using_ffx()

        self.ffx_obj.notify_intentional_disconnect.assert_called_once()
