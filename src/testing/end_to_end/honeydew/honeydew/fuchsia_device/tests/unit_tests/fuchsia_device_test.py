# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.fuchsia_device.fuchsia_device.py."""

import base64
import ipaddress
import json
import os
import unittest
from collections.abc import Callable
from typing import Any
from unittest import mock

import fidl_fuchsia_buildinfo as f_buildinfo
import fidl_fuchsia_developer_remotecontrol as fd_remotecontrol
import fidl_fuchsia_feedback as f_feedback
import fidl_fuchsia_hardware_power_statecontrol as fhp_statecontrol
import fidl_fuchsia_hwinfo as f_hwinfo
import fidl_fuchsia_io as f_io
import fuchsia_controller_py as fuchsia_controller
import fuchsia_inspect
import honeydew.affordances.connectivity.wlan.core as wlan_core
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable, errors
from honeydew.affordances import location
from honeydew.affordances.connectivity.bluetooth.avrcp import avrcp_using_sl4f
from honeydew.affordances.connectivity.bluetooth.gap import gap_using_fc
from honeydew.affordances.connectivity.wlan import wlan_policy
from honeydew.affordances.drivers.fake_battery import fake_battery_using_ffx
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller_using_starnix,
)
from honeydew.affordances.rtc import rtc_using_fc
from honeydew.affordances.session import session_using_ffx
from honeydew.affordances.starnix import starnix_using_ffx
from honeydew.affordances.tracing import tracing_using_fc
from honeydew.affordances.ui.screenshot import screenshot_using_ffx
from honeydew.affordances.ui.user_input import user_input_using_fc
from honeydew.affordances.virtual_audio import audio_using_fuchsia_controller
from honeydew.auxiliary_devices.power_switch import (
    power_switch as power_switch_interface,
)
from honeydew.auxiliary_devices.usb_power_hub import (
    usb_power_hub as usb_power_hub_interface,
)
from honeydew.fuchsia_device import fuchsia_device
from honeydew.transports.adb import adb as adb_transport
from honeydew.transports.fastboot import fastboot
from honeydew.transports.ffx import config as ffx_config
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx
from honeydew.transports.fuchsia_controller import errors as fc_errors
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.transports.serial import serial as serial_interface
from honeydew.transports.serial import serial_using_unix_socket
from honeydew.transports.sl4f import sl4f as sl4f_transport
from honeydew.typing import custom_types
from parameterized import param, parameterized

# pylint: disable=protected-access

_INSPECT_DATA_JSON_TEXT = """
[
  {
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo",
        "timestamp": 181016000000000,
        "file_name": "foo.txt"
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  },
  {
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo2",
        "timestamp": 181016000000000
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 1
  },
  {
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo2",
        "timestamp": 181016000000000,
        "errors": [
          {
            "message": "Unknown failure"
          }
        ]
    },
    "moniker": "core/example",
    "payload": null,
    "version": 1
  }
]
"""

_INSPECT_DATA_BAD_VERSION = """
{
    "data_source": "Inspect",
    "metadata": {
        "component_url": "foo",
        "timestamp": 181016000000000,
        "file_name": "foo.txt"
    },
    "moniker": "core/example",
    "payload": {
      "root": {
        "value": 100
      }
    },
    "version": 2
  }
"""

_IPV6: str = "fe80::4fce:3102:ef13:888c%qemu"
_IPV6_OBJ: ipaddress.IPv6Address = ipaddress.IPv6Address(_IPV6)

_SSH_ADDRESS: ipaddress.IPv6Address = _IPV6_OBJ
_SSH_PORT = 8022
_TARGET_SSH_ADDRESS = custom_types.IpPort(ip=_SSH_ADDRESS, port=_SSH_PORT)

_INPUT_ARGS: dict[str, Any] = {
    "device_name": "fuchsia-emulator",
    "device_ip": _TARGET_SSH_ADDRESS,
    "device_serial_socket": "/tmp/socket",
    "ffx_config_data": ffx_config.FfxConfigData(
        isolate_dir=fuchsia_controller.IsolateDir("/tmp/isolate"),
        logs_dir="/tmp/logs",
        binary_path="/bin/ffx",
        logs_level="debug",
        enable_usb=False,
        usb_socket_path=None,
        usb_driver_autostart=False,
        subtools_search_path=None,
        proxy_timeout_secs=None,
        ssh_keepalive_timeout=None,
        emu_instance_dir=None,
        ssh_private_keys=None,
        ssh_public_keys=None,
    ),
}


_MOCK_ADDRESS = json.dumps(
    [
        {
            "nodename": "fuchsia-emulator",
            "rcs_state": "Y",
            "serial": "<unknown>",
            "target_type": "core.x64",
            "target_state": "Product",
            "addresses": [
                {"type": "Ip", "ip": str(_SSH_ADDRESS), "ssh_port": _SSH_PORT}
            ],
            "is_default": False,
            "is_manual": False,
        }
    ]
)

_MOCK_ARGS: dict[str, str] = {
    "board": "x64",
    "product": "core",
    "INSPECT_DATA_JSON_TEXT": _INSPECT_DATA_JSON_TEXT,
    "INSPECT_DATA_BAD_VERSION": _INSPECT_DATA_BAD_VERSION,
    "ffx_target_ssh_address_output": f"{_MOCK_ADDRESS}",
}

_BASE64_ENCODED_BYTES: bytes = base64.b64decode("some base64 encoded string==")

_MOCK_BUILD_INFO = f_buildinfo.BuildInfo(
    version="123456",
)

_MOCK_DEVICE_INFO = f_hwinfo.DeviceInfo(
    serial_number="123456",
)

_MOCK_PRODUCT_INFO = f_hwinfo.ProductInfo(
    manufacturer="default-manufacturer",
    model="default-model",
    name="default-product-name",
)


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom test name function method."""
    test_func_name: str = testcase_func.__name__
    test_label: str

    try:
        params_dict: dict[str, Any] = param_arg.args[0]
        test_label = parameterized.to_safe_name(params_dict["label"])
    except Exception:  # pylint: disable=broad-except
        test_label = parameterized.to_safe_name(param_arg.kwargs["label"])

    return f"{test_func_name}_with_{test_label}"


def _file_read_result(data: f_io.Transfer) -> f_io.ReadableReadResult:
    return f_io.ReadableReadResult(
        response=f_io.ReadableReadResponse(data=data)
    )


def _file_attr_resp(
    status: ZxStatus, size: int
) -> f_io.NodeGetAttributesResult:
    if status.raw() != ZxStatus.ZX_OK:
        return f_io.NodeGetAttributesResult(err=status.raw())
    else:
        return f_io.NodeGetAttributesResult(
            response=f_io.NodeAttributes2(
                # The args below (besides content_size) are arbitrary.
                mutable_attributes=f_io.MutableNodeAttributes(
                    mode=0,
                    creation_time=0,
                    modification_time=0,
                ),
                immutable_attributes=f_io.ImmutableNodeAttributes(
                    content_size=size,
                    id_=0,
                    storage_size=0,
                    link_count=0,
                ),
            ),
        )


class FuchsiaDeviceTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.fuchsia_device.fuchsia_device.py."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        self.fd_fc_obj: fuchsia_device.FuchsiaDevice
        super().__init__(*args, **kwargs)

    def setUp(self) -> None:
        with (
            mock.patch.object(
                ffx.FFX,
                "_check_running_monitor",
                return_value=False,
                autospec=True,
            ),
            mock.patch.object(
                fc_transport.FuchsiaController,
                "create_context",
                autospec=True,
            ) as mock_fc_create_context,
            mock.patch.object(
                ffx.FFX,
                "check_connection",
                autospec=True,
            ) as mock_ffx_check_connection,
            mock.patch.object(
                fc_transport.FuchsiaController,
                "check_connection",
                autospec=True,
            ) as mock_fc_check_connection,
            mock.patch.object(
                sl4f_transport.SL4F,
                "start_server",
                autospec=True,
            ) as mock_sl4f_start_server,
            mock.patch.object(
                sl4f_transport.SL4F,
                "check_connection",
                autospec=True,
            ) as mock_sl4f_check_connection,
        ):
            sync_fd_fc_obj = fuchsia_device.FuchsiaDevice(
                device_info=custom_types.DeviceInfo(
                    name=_INPUT_ARGS["device_name"],
                    serial_number=None,
                    ip_port=_INPUT_ARGS["device_ip"],
                    serial_socket=_INPUT_ARGS["device_serial_socket"],
                ),
                ffx_config_data=_INPUT_ARGS["ffx_config_data"],
                config={
                    "affordances": {
                        "bluetooth": {
                            "implementation": "fuchsia-controller",
                        },
                    }
                },
            )
            self.fd_fc_obj = sync_fd_fc_obj
            mock_ctx = mock.MagicMock()
            mock_ctx.channel_create.return_value = (
                mock.MagicMock(),
                mock.MagicMock(),
            )
            self.fd_fc_obj.fuchsia_controller.ctx = mock_ctx

            mock_fc_create_context.assert_called_once_with(
                self.fd_fc_obj.fuchsia_controller
            )
            mock_fc_check_connection.assert_called()
            mock_ffx_check_connection.assert_called()
            mock_sl4f_start_server.assert_not_called()
            mock_sl4f_check_connection.assert_not_called()

        with (
            mock.patch.object(
                ffx.FFX,
                "_check_running_monitor",
                return_value=False,
                autospec=True,
            ),
            mock.patch.object(
                fc_transport.FuchsiaController,
                "create_context",
                autospec=True,
            ) as mock_fc_create_context,
            mock.patch.object(
                ffx.FFX,
                "check_connection",
                autospec=True,
            ) as mock_ffx_check_connection,
            mock.patch.object(
                fc_transport.FuchsiaController,
                "check_connection",
                autospec=True,
            ) as mock_fc_check_connection,
            mock.patch.object(
                sl4f_transport.SL4F,
                "start_server",
                autospec=True,
            ) as mock_sl4f_start_server,
            mock.patch.object(
                sl4f_transport.SL4F,
                "check_connection",
                autospec=True,
            ) as mock_sl4f_check_connection,
        ):
            sync_fd_sl4f_obj = fuchsia_device.FuchsiaDevice(
                device_info=custom_types.DeviceInfo(
                    name=_INPUT_ARGS["device_name"],
                    serial_number=None,
                    ip_port=None,
                    serial_socket=_INPUT_ARGS["device_serial_socket"],
                ),
                ffx_config_data=_INPUT_ARGS["ffx_config_data"],
                config={
                    "affordances": {
                        "bluetooth": {
                            "implementation": "sl4f",
                        },
                    }
                },
            )
            self.fd_sl4f_obj = sync_fd_sl4f_obj
            mock_fc_create_context.assert_called_once_with(
                self.fd_sl4f_obj.fuchsia_controller
            )
            mock_fc_check_connection.assert_called()
            mock_ffx_check_connection.assert_called()
            mock_sl4f_start_server.assert_called_once_with(
                self.fd_sl4f_obj.sl4f
            )
            mock_sl4f_check_connection.assert_called()

    # List all the tests related to __init__
    def test_device_is_a_fuchsia_device(self) -> None:
        """Test case to make sure DUT is a fuchsia device"""
        self.assertIsInstance(self.fd_fc_obj, fuchsia_device.FuchsiaDevice)
        self.assertIsInstance(self.fd_sl4f_obj, fuchsia_device.FuchsiaDevice)

    # List all the tests related to transports
    @mock.patch.object(
        adb_transport.Adb,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_adb_transport(self, mock_adb_init: mock.Mock) -> None:
        """Test case to make sure fuchsia_device supports adb transport."""
        self.assertIsInstance(
            self.fd_fc_obj.adb,
            adb_transport.Adb,
        )
        mock_adb_init.assert_called_once()

    def test_adb_transport_disabled(self) -> None:
        """Test case to make sure fuchsia_device raises NotEnabledError when adb is disabled."""
        config = {
            "transports": {
                "adb": {
                    "enabled": False,
                }
            }
        }
        with (
            mock.patch.object(
                ffx.FFX,
                "check_connection",
                autospec=True,
            ),
            mock.patch.object(
                fc_transport.FuchsiaController,
                "check_connection",
                autospec=True,
            ),
            mock.patch.object(
                fc_transport.FuchsiaController,
                "create_context",
                autospec=True,
            ),
        ):
            fd_obj = fuchsia_device.FuchsiaDevice(
                device_info=custom_types.DeviceInfo(
                    name=_INPUT_ARGS["device_name"],
                    serial_number=None,
                    ip_port=_INPUT_ARGS["device_ip"],
                    serial_socket=_INPUT_ARGS["device_serial_socket"],
                ),
                ffx_config_data=_INPUT_ARGS["ffx_config_data"],
                config=config,
            )
            with self.assertRaises(errors.NotEnabledError):
                _ = fd_obj.adb

    @mock.patch.object(
        fastboot.Fastboot,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_fastboot_transport(self, mock_fastboot_init: mock.Mock) -> None:
        """Test case to make sure fuchsia_device supports fastboot
        transport."""
        self.assertIsInstance(
            self.fd_fc_obj.fastboot,
            fastboot.Fastboot,
        )
        mock_fastboot_init.assert_called_once()

    def test_ffx_transport(self) -> None:
        """Test case to make sure fuchsia_device supports ffx transport."""
        self.assertIsInstance(
            self.fd_fc_obj.ffx,
            ffx.FFX,
        )

    def test_ffx_transport_with_shared_data(self) -> None:
        """Test case to make sure fuchsia_device supports ffx transport with shared_data."""
        shared_data = "/tmp/shared_data"
        config = {
            "transports": {
                "ffx": {
                    "shared_data": shared_data,
                }
            }
        }
        with (
            mock.patch.object(
                ffx.FFX,
                "check_connection",
                autospec=True,
            ) as mock_ffx_check_connection,
            mock.patch.object(
                fc_transport.FuchsiaController,
                "check_connection",
                autospec=True,
            ),
            mock.patch.object(
                fc_transport.FuchsiaController,
                "create_context",
                autospec=True,
            ),
        ):
            fd_obj = fuchsia_device.FuchsiaDevice(
                device_info=custom_types.DeviceInfo(
                    name=_INPUT_ARGS["device_name"],
                    serial_number=None,
                    ip_port=_INPUT_ARGS["device_ip"],
                    serial_socket=_INPUT_ARGS["device_serial_socket"],
                ),
                ffx_config_data=_INPUT_ARGS["ffx_config_data"],
                config=config,
            )
            ffx_obj = fd_obj.ffx
            self.assertIsInstance(ffx_obj, ffx.FFX)
            self.assertEqual(ffx_obj.shared_data, shared_data)
            mock_ffx_check_connection.assert_called()

    def test_sl4f_impl(self) -> None:
        """Test case to make sure fuchsia_device does not support sl4f
        transport."""
        with (
            mock.patch.object(
                sl4f_transport.SL4F,
                "start_server",
                autospec=True,
            ) as mock_sl4f_start_server,
        ):
            self.assertIsInstance(self.fd_fc_obj.sl4f, sl4f_transport.SL4F)
            mock_sl4f_start_server.assert_called_once_with(self.fd_fc_obj.sl4f)

        self.assertIsInstance(self.fd_sl4f_obj.sl4f, sl4f_transport.SL4F)

    def test_fuchsia_controller_transport(self) -> None:
        """Test case to make sure fuchsia_device supports fuchsia-controller
        transport."""
        self.assertIsInstance(
            self.fd_fc_obj.fuchsia_controller,
            fc_transport.FuchsiaController,
        )

    def test_serial_transport(self) -> None:
        """Test case to make sure fuchsia_device supports serial transport."""
        self.assertIsInstance(
            self.fd_fc_obj.serial,
            serial_using_unix_socket.SerialUsingUnixSocket,
        )

    def test_serial_transport_error(self) -> None:
        """Test case to make sure fuchsia_device raises error when we try to
        access "serial" transport without serial_socket."""

        device_info: custom_types.DeviceInfo = self.fd_fc_obj._device_info

        self.fd_fc_obj._device_info = custom_types.DeviceInfo(
            name=_INPUT_ARGS["device_name"],
            serial_number=None,
            ip_port=None,
            serial_socket=None,
        )

        with self.assertRaisesRegex(
            errors.FuchsiaDeviceError,
            "'serial_socket' arg need to be provided during the init to use Serial affordance",
        ):
            _: serial_interface.Serial = self.fd_fc_obj.serial

        self.fd_fc_obj._device_info = device_info

    # List all the tests related to affordances
    def test_session(self) -> None:
        """Test case to make sure fuchsia_device supports session
        affordance implemented using FFX"""
        self.assertIsInstance(
            self.fd_fc_obj.session, session_using_ffx.SessionUsingFfx
        )

    def test_screenshot(self) -> None:
        """Test case to make sure fuchsia_device supports screenshot
        affordance implemented using FFX"""
        self.assertIsInstance(
            self.fd_fc_obj.screenshot,
            screenshot_using_ffx.ScreenshotUsingFfx,
        )

    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value="test.hardwarepowercontrol.Service",
        autospec=True,
    )
    def test_fake_battery(self, mock_ffx_run: mock.Mock) -> None:
        """Test case to make sure fuchsia_device supports fake_battery
        affordance implemented using FFX"""
        self.assertIsInstance(
            self.fd_fc_obj.fake_battery,
            fake_battery_using_ffx.FakeBatteryUsingFfx,
        )

    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    def test_audio(self, mock_ffx_run: mock.Mock) -> None:
        """Test case to make sure fuchsia_device supports audio
        affordance implemented using Fuchsia controller"""
        self.fd_fc_obj.fuchsia_controller.ctx = mock.Mock()

        mock_ffx_run.return_value = (
            '{"instances": [{"moniker": "core/audio_recording"}]}'
        )

        self.assertIsInstance(
            self.fd_fc_obj.virtual_audio,
            audio_using_fuchsia_controller.VirtualAudioUsingFuchsiaController,
        )

    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    def test_audio_not_supported(self, mock_ffx_run: mock.Mock) -> None:
        """Test case to make sure fuchsia_device raises NotSupportedError
        when audio affordance is not supported"""
        self.fd_fc_obj.fuchsia_controller.ctx = mock.Mock()

        mock_ffx_run.return_value = (
            '{"instances": [{"moniker": "core/some_other_component"}, '
            '{"test": "test_value"}]}'
        )

        with self.assertRaisesRegex(
            errors.NotSupportedError,
            "core/audio_recording is not available in device fuchsia-emulator",
        ):
            self.fd_fc_obj.virtual_audio  # pylint: disable=pointless-statement

    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value="core/starnix_runner/kernels:",
        autospec=True,
    )
    def test_system_power_state_controller(
        self,
        mock_ffx_run: mock.Mock,
    ) -> None:
        """Test case to make sure fuchsia_device supports
        system_power_state_controller affordance implemented using starnix"""
        self.assertIsInstance(
            self.fd_fc_obj.system_power_state_controller,
            system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        )
        mock_ffx_run.assert_called_once()

    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value="core/starnix_runner/kernels:",
        autospec=True,
    )
    def test_starnix(
        self,
        mock_ffx_run: mock.Mock,
    ) -> None:
        """Test case to make sure fuchsia_device supports
        starnix affordance implemented using starnix"""
        self.assertIsInstance(
            self.fd_fc_obj.starnix, starnix_using_ffx.StarnixUsingFfx
        )
        mock_ffx_run.assert_called_once()

    @mock.patch.object(
        rtc_using_fc.RtcUsingFc,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_rtc(self, mock_rtc_fc_init: mock.Mock) -> None:
        """Test case to make sure fuchsia_device supports rtc affordance
        implemented using fuchsia-controller"""
        self.assertIsInstance(
            self.fd_fc_obj.rtc,
            rtc_using_fc.RtcUsingFc,
        )
        mock_rtc_fc_init.assert_called_once_with(
            self.fd_fc_obj.rtc,
            fuchsia_controller=self.fd_fc_obj.fuchsia_controller,
            reboot_affordance=self.fd_fc_obj,
        )

    def test_tracing(self) -> None:
        """Test case to make sure fuchsia_device supports tracing affordance
        implemented using fuchsia-controller"""
        self.assertIsInstance(
            self.fd_fc_obj.tracing,
            tracing_using_fc.TracingUsingFc,
        )

    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    def test_user_input(
        self,
        mock_ffx_run: mock.Mock,
    ) -> None:
        """Test case to make sure fuchsia_device supports
        user_input affordance."""

        moniker = (
            user_input_using_fc._INPUT_HELPER_COMPONENT
        )  # pylint: disable=protected-access
        mock_output = {"instances": [{"moniker": moniker}]}
        mock_ffx_run.return_value = json.dumps(mock_output)

        self.assertIsInstance(
            self.fd_fc_obj.user_input,
            user_input_using_fc.UserInputUsingFc,
        )

    def test_bluetooth_avrcp_fc_transport(self) -> None:
        """Test case to make sure fuchsia_device only supports
        SL4F based bluetooth_avrcp affordance."""
        with self.assertRaises(NotImplementedError):
            self.fd_fc_obj.bluetooth_avrcp  # pylint: disable=pointless-statement

    @mock.patch.object(
        avrcp_using_sl4f.AvrcpUsingSl4f,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_bluetooth_avrcp_sl4f_impl(
        self, mock_avrcp_init: mock.Mock
    ) -> None:
        """Test case to make sure fuchsia_device only supports
        SL4F based bluetooth_avrcp affordance."""
        self.assertIsInstance(
            self.fd_sl4f_obj.bluetooth_avrcp,
            avrcp_using_sl4f.AvrcpUsingSl4f,
        )
        mock_avrcp_init.assert_called_once()

    @mock.patch.object(
        gap_using_fc.GapUsingFc,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_bluetooth_gap_fc(self, bt_gap_fc_init: mock.Mock) -> None:
        """Test case to make sure fuchsia_device supports
        Fuchsia-Controller based bluetooth_gap affordance."""
        self.assertIsInstance(
            self.fd_fc_obj.bluetooth_gap,
            gap_using_fc.GapUsingFc,
        )
        bt_gap_fc_init.assert_called_once_with(
            self.fd_fc_obj.bluetooth_gap,
            device_name=self.fd_fc_obj._device_info.name,
            fuchsia_controller=self.fd_fc_obj.fuchsia_controller,
            reboot_affordance=self.fd_fc_obj,
        )

    @mock.patch.object(
        location.Location,
        "__init__",
        autospec=True,
        return_value=None,
    )
    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value="".join(wlan_policy._REQUIRED_CAPABILITIES),
        autospec=True,
    )
    @mock.patch.object(
        wlan_policy.WlanPolicy,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_wlan_policy(
        self,
        wlan_policy_init: mock.Mock,
        # pylint: disable-next=unused-argument
        mock_ffx_run: mock.Mock,
        # pylint: disable-next=unused-argument
        location_init: mock.Mock,
    ) -> None:
        """Test case to make sure fuchsia_device supports Fuchsia-Controller based wlan_policy
        affordance."""
        self.assertIsInstance(
            self.fd_fc_obj.wlan_policy,
            wlan_policy.WlanPolicy,
        )
        wlan_policy_init.assert_called_once_with(
            self.fd_fc_obj.wlan_policy,
            device_name=self.fd_fc_obj._device_info.name,
            ffx=self.fd_fc_obj.ffx,
            fuchsia_controller=self.fd_fc_obj.fuchsia_controller,
            reboot_affordance=self.fd_fc_obj,
            fuchsia_device_close=self.fd_fc_obj,
            location=self.fd_fc_obj.location,
        )

    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    @mock.patch.object(
        wlan_core.WlanCore,
        "__init__",
        autospec=True,
        return_value=None,
    )
    def test_wlan_core(
        self,
        wlan_core_init: mock.Mock,
        # pylint: disable-next=unused-argument
        mock_ffx_run: mock.Mock,
    ) -> None:
        """Test case to make sure fuchsia_device supports Fuchsia-Controller based wlan
        affordance."""
        self.assertIsInstance(
            self.fd_fc_obj.wlan_core,
            wlan_core.WlanCore,
        )
        wlan_core_init.assert_called_once_with(
            self.fd_fc_obj.wlan_core,
            device_name=self.fd_fc_obj._device_info.name,
            ffx=self.fd_fc_obj.ffx,
            fuchsia_controller=self.fd_fc_obj.fuchsia_controller,
            reboot_affordance=self.fd_fc_obj,
            fuchsia_device_close=self.fd_fc_obj,
        )

    # List all the tests related to static properties
    @mock.patch.object(
        ffx.FFX,
        "get_target_board",
        return_value=_MOCK_ARGS["board"],
        autospec=True,
    )
    def test_board(self, mock_ffx_get_target_board: mock.Mock) -> None:
        """Testcase for FuchsiaDevice.board property"""
        self.assertEqual(self.fd_fc_obj.board, _MOCK_ARGS["board"])
        mock_ffx_get_target_board.assert_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_product_info",
        return_value={
            "manufacturer": "default-manufacturer",
            "model": "default-model",
            "name": "default-product-name",
        },
        autospec=True,
    )
    async def test_manufacturer(self, *unused_args: Any) -> None:
        """Testcase for FuchsiaDevice.manufacturer property"""
        self.assertEqual(
            await self.fd_fc_obj.manufacturer(), "default-manufacturer"
        )

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_product_info",
        return_value={
            "manufacturer": "default-manufacturer",
            "model": "default-model",
            "name": "default-product-name",
        },
        autospec=True,
    )
    async def test_model(self, *unused_args: Any) -> None:
        """Testcase for FuchsiaDevice.model property"""
        self.assertEqual(await self.fd_fc_obj.model(), "default-model")

    @mock.patch.object(
        ffx.FFX,
        "get_target_product",
        return_value=_MOCK_ARGS["product"],
        autospec=True,
    )
    def test_product(self, mock_ffx_get_target_product: mock.Mock) -> None:
        """Testcase for FuchsiaDevice.product property"""
        self.assertEqual(self.fd_fc_obj.product, _MOCK_ARGS["product"])
        mock_ffx_get_target_product.assert_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_product_info",
        return_value={
            "manufacturer": "default-manufacturer",
            "model": "default-model",
            "name": "default-product-name",
        },
        autospec=True,
    )
    async def test_product_name(self, *unused_args: Any) -> None:
        """Testcase for FuchsiaDevice.product_name property"""
        self.assertEqual(
            await self.fd_fc_obj.product_name(), "default-product-name"
        )

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_device_info_from_fidl",
        return_value={
            "serial_number": "default-serial-number",
        },
        autospec=True,
    )
    async def test_serial_number(self, *unused_args: Any) -> None:
        """Testcase for FuchsiaDevice.serial_number property"""
        self.assertEqual(
            await self.fd_fc_obj.serial_number(), "default-serial-number"
        )

    # List all the tests related to dynamic properties
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_build_info",
        return_value={
            "version": "1.2.3",
        },
        autospec=True,
    )
    async def test_firmware_version(self, *unused_args: Any) -> None:
        """Testcase for FuchsiaDevice.firmware_version property"""
        self.assertEqual(await self.fd_fc_obj.firmware_version(), "1.2.3")

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_last_reboot_info",
        return_value={
            "reason": 9,
        },
        autospec=True,
    )
    async def test_last_reboot_reason(self, *unused_args: Any) -> None:
        """Testcase for FuchsiaDevice.last_reboot_reason property"""
        self.assertEqual(
            await self.fd_fc_obj.last_reboot_reason(), "USER_REQUEST"
        )

    # List all the tests related to affordances
    def test_fuchsia_device_is_reboot_capable(self) -> None:
        """Test case to make sure fuchsia device is reboot capable"""
        self.assertIsInstance(
            self.fd_fc_obj, affordances_capable.RebootCapableDevice
        )

    # List all the tests related to public methods
    @parameterized.expand(
        [
            (
                {
                    "label": "no_register_for_on_device_close",
                    "register_for_on_device_close": None,
                    "expected_exception": False,
                },
            ),
            (
                {
                    "label": "register_for_on_device_close_fn_returning_success",
                    "register_for_on_device_close": lambda: None,
                    "expected_exception": False,
                },
            ),
            (
                {
                    "label": "register_for_on_device_close_fn_returning_exception",
                    "register_for_on_device_close": lambda: 1 / 0,
                    "expected_exception": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_close(
        self,
        parameterized_dict: dict[str, Any],
    ) -> None:
        """Testcase for FuchsiaDevice.close()"""
        # Reset the `_on_device_close_fns` variable at the beginning of the test
        self.fd_fc_obj._on_device_close_fns = []

        if parameterized_dict["register_for_on_device_close"]:
            self.fd_fc_obj.register_for_on_device_close(
                parameterized_dict["register_for_on_device_close"]
            )
        if parameterized_dict["expected_exception"]:
            with self.assertRaises(Exception):
                await self.fd_fc_obj.close()
        else:
            await self.fd_fc_obj.close()

        # Reset the `_on_device_close_fns` variable at the end of the test
        self.fd_fc_obj._on_device_close_fns = []

    @mock.patch.object(
        sl4f_transport.SL4F,
        "check_connection",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "check_connection",
        autospec=True,
    )
    @mock.patch.object(ffx.FFX, "check_connection", autospec=True)
    def test_health_check_fc(
        self,
        mock_ffx_check_connection: mock.Mock,
        mock_fc_check_connection: mock.Mock,
        mock_sl4f_check_connection: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.health_check() when transport is set to
        Fuchsia-Controller"""
        self.fd_fc_obj.health_check()
        mock_ffx_check_connection.assert_called_once_with(self.fd_fc_obj.ffx)
        mock_fc_check_connection.assert_called_once_with(
            self.fd_fc_obj.fuchsia_controller
        )
        mock_sl4f_check_connection.assert_not_called()

    @mock.patch.object(
        sl4f_transport.SL4F,
        "check_connection",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "check_connection",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "check_connection",
        autospec=True,
    )
    def test_health_check_sl4f(
        self,
        mock_ffx_check_connection: mock.Mock,
        mock_fc_check_connection: mock.Mock,
        mock_sl4f_check_connection: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.health_check() when transport is set to
        Fuchsia-Controller-Preferred"""
        self.fd_sl4f_obj.health_check()

        mock_ffx_check_connection.assert_called_once_with(self.fd_sl4f_obj.ffx)
        mock_fc_check_connection.assert_called_once_with(
            self.fd_sl4f_obj.fuchsia_controller
        )
        mock_sl4f_check_connection.assert_called_once_with(
            self.fd_sl4f_obj.sl4f
        )

    @mock.patch.object(
        fc_transport.FuchsiaController,
        "check_connection",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "check_connection",
        side_effect=ffx_errors.FfxConnectionError("ffx connection error"),
        autospec=True,
    )
    def test_health_check_exception(
        self,
        mock_ffx_check_connection: mock.Mock,
        mock_fc_check_connection: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.health_check() raising HealthCheckError"""
        with self.assertRaises(errors.HealthCheckError):
            self.fd_fc_obj.health_check()

        mock_ffx_check_connection.assert_called_once_with(self.fd_fc_obj.ffx)
        mock_fc_check_connection.assert_not_called()

    @parameterized.expand(
        [
            param(
                label="without_selectors_and_monikers",
                selectors=None,
                monikers=None,
                expected_cmd=[
                    "--machine",
                    "json",
                    "inspect",
                    "show",
                ],
            ),
            param(
                label="with_one_selector",
                selectors=["selector1"],
                monikers=None,
                expected_cmd=[
                    "--machine",
                    "json",
                    "inspect",
                    "show",
                    "selector1",
                ],
            ),
            param(
                label="with_two_selectors",
                selectors=["selector1", "selector2"],
                monikers=None,
                expected_cmd=[
                    "--machine",
                    "json",
                    "inspect",
                    "show",
                    "selector1",
                    "selector2",
                ],
            ),
            param(
                label="with_one_moniker",
                selectors=None,
                monikers=["core/coll:bar"],
                expected_cmd=[
                    "--machine",
                    "json",
                    "inspect",
                    "show",
                    r"core/coll\:bar",
                ],
            ),
            param(
                label="with_one_selector_and_one_moniker",
                selectors=["selector1"],
                monikers=["core/coll:bar"],
                expected_cmd=[
                    "--machine",
                    "json",
                    "inspect",
                    "show",
                    "selector1",
                    r"core/coll\:bar",
                ],
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    def test_get_inspect_data(
        self,
        mock_ffx_run: mock.Mock,
        label: str,  # pylint: disable=unused-argument
        selectors: list[str],
        monikers: list[str],
        expected_cmd: list[str],
    ) -> None:
        """Test case for get_inspect_data()"""
        mock_ffx_run.return_value = _MOCK_ARGS["INSPECT_DATA_JSON_TEXT"]

        inspect_data_collection: fuchsia_inspect.InspectDataCollection = (
            self.fd_fc_obj.get_inspect_data(
                selectors=selectors,
                monikers=monikers,
            )
        )

        self.assertIsInstance(
            inspect_data_collection, fuchsia_inspect.InspectDataCollection
        )
        for inspect_data in inspect_data_collection.data:
            self.assertIsInstance(inspect_data, fuchsia_inspect.InspectData)

        mock_ffx_run.assert_called_with(
            mock.ANY,
            cmd=expected_cmd,
            log_output=False,
        )

    @parameterized.expand(
        [
            param(
                label="with_FfxCommandError",
                side_effect=ffx_errors.FfxCommandError("error"),
                expected_error=errors.InspectError,
            ),
            param(
                label="with_DeviceNotConnectedError",
                side_effect=errors.DeviceNotConnectedError("error"),
                expected_error=errors.InspectError,
            ),
            param(
                label="with_someother_error",
                side_effect=ffx_errors.FfxTimeoutError("error"),
                expected_error=ffx_errors.FfxTimeoutError,
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    def test_get_inspect_data_exception_when_ffx_run_fails(
        self,
        mock_ffx_run: mock.Mock,
        label: str,  # pylint: disable=unused-argument,
        side_effect: type[errors.HoneydewError],
        expected_error: type[errors.HoneydewError],
    ) -> None:
        """Test case for get_inspect_data() raising InspectError failure."""
        mock_ffx_run.side_effect = side_effect

        with self.assertRaises(expected_error):
            self.fd_fc_obj.get_inspect_data()

        mock_ffx_run.assert_called_once()

    @mock.patch.object(
        ffx.FFX,
        "run",
        autospec=True,
    )
    def test_get_inspect_data_exception_when_inspect_data_parsing_fails(
        self,
        mock_ffx_run: mock.Mock,
    ) -> None:
        """Test case for get_inspect_data() raising InspectError failure."""
        mock_ffx_run.return_value = _MOCK_ARGS["INSPECT_DATA_BAD_VERSION"]

        with self.assertRaises(errors.InspectError):
            self.fd_fc_obj.get_inspect_data()

        mock_ffx_run.assert_called_once()

    @parameterized.expand(
        [
            (
                {
                    "label": "info_level",
                    "log_level": custom_types.LEVEL.INFO,
                    "log_message": "info message",
                },
            ),
            (
                {
                    "label": "warning_level",
                    "log_level": custom_types.LEVEL.WARNING,
                    "log_message": "warning message",
                },
            ),
            (
                {
                    "label": "error_level",
                    "log_level": custom_types.LEVEL.ERROR,
                    "log_message": "error message",
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_send_log_command",
        autospec=True,
    )
    async def test_log_message_to_device(
        self,
        parameterized_dict: dict[str, Any],
        mock_send_log_command: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.log_message_to_device()"""
        await self.fd_fc_obj.log_message_to_device(
            level=parameterized_dict["log_level"],
            message=parameterized_dict["log_message"],
        )

        mock_send_log_command.assert_called_with(
            self.fd_fc_obj,
            tag="lacewing",
            message=mock.ANY,
            level=parameterized_dict["log_level"],
        )

    @parameterized.expand(
        [
            (
                {
                    "label": "no_register_for_on_device_boot",
                    "register_for_on_device_boot": None,
                    "expected_exception": False,
                },
            ),
            (
                {
                    "label": "register_for_on_device_boot_fn_returning_success",
                    "register_for_on_device_boot": lambda: None,
                    "expected_exception": False,
                },
            ),
            (
                {
                    "label": "register_for_on_device_boot_fn_returning_exception",
                    "register_for_on_device_boot": lambda: 1 / 0,
                    "expected_exception": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    @mock.patch.object(
        sl4f_transport.SL4F,
        "start_server",
        autospec=True,
    )
    async def test_on_device_boot_fc(
        self,
        parameterized_dict: dict[str, Any],
        mock_sl4f_start_server: mock.Mock,
        mock_fc_create_context: mock.Mock,
        mock_health_check: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.on_device_boot() when transport is set to
        Fuchsia-Controller"""
        # Reset the `_on_device_boot_fns` variable at the beginning of the test
        self.fd_fc_obj._on_device_boot_fns = []

        if parameterized_dict["register_for_on_device_boot"]:
            self.fd_fc_obj.register_for_on_device_boot(
                parameterized_dict["register_for_on_device_boot"]
            )
        if parameterized_dict["expected_exception"]:
            with self.assertRaises(Exception):
                await self.fd_fc_obj.on_device_boot()
        else:
            await self.fd_fc_obj.on_device_boot()

        # Reset the `_on_device_boot_fns` variable at the end of the test
        self.fd_fc_obj._on_device_boot_fns = []

        mock_fc_create_context.assert_called_once()
        mock_health_check.assert_called_once()
        mock_sl4f_start_server.assert_not_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    @mock.patch.object(
        sl4f_transport.SL4F,
        "start_server",
        autospec=True,
    )
    async def test_on_device_boot(
        self,
        mock_sl4f_start_server: mock.Mock,
        mock_fc_create_context: mock.Mock,
        mock_sl4f_health_check: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.on_device_boot() when transport is set to
        Fuchsia-Controller-Preferred"""
        await self.fd_sl4f_obj.on_device_boot()

        mock_sl4f_start_server.assert_called_once_with(self.fd_sl4f_obj.sl4f)
        mock_fc_create_context.assert_called_once_with(
            self.fd_sl4f_obj.fuchsia_controller
        )
        mock_sl4f_health_check.assert_called_once_with(self.fd_sl4f_obj)

    def test_set_power_switch(self) -> None:
        """Testcase for FuchsiaDevice.set_power_switch()"""
        power_switch = mock.MagicMock(spec=power_switch_interface.PowerSwitch)
        self.fd_fc_obj.set_power_switch(power_switch=power_switch, outlet=5)
        self.assertEqual(self.fd_fc_obj._power_switch, power_switch)
        self.assertEqual(self.fd_fc_obj._power_switch_outlet, 5)

    async def test_power_cycle_not_supported_error(self) -> None:
        """Testcase for FuchsiaDevice.power_cycle() raising NotSupportedError."""
        self.fd_fc_obj._power_switch = None
        with self.assertRaises(errors.NotSupportedError):
            await self.fd_fc_obj.power_cycle()

    @mock.patch.object(
        ffx.FFX,
        "notify_intentional_disconnect",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "on_device_boot",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "wait_for_online",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "wait_for_offline",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "log_message_to_device",
        autospec=True,
    )
    async def test_power_cycle(
        self,
        mock_log_message_to_device: mock.Mock,
        mock_wait_for_offline: mock.Mock,
        mock_wait_for_online: mock.Mock,
        mock_on_device_boot: mock.Mock,
        mock_ffx_notify_intentional_disconnect: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.power_cycle()"""
        power_switch = mock.MagicMock(spec=power_switch_interface.PowerSwitch)
        self.fd_fc_obj.set_power_switch(power_switch=power_switch, outlet=5)
        await self.fd_fc_obj.power_cycle()

        self.assertEqual(mock_log_message_to_device.call_count, 2)
        power_switch.power_off.assert_called_with(5)
        power_switch.power_on.assert_called_with(5)
        mock_ffx_notify_intentional_disconnect.assert_called()
        mock_wait_for_offline.assert_called()
        mock_wait_for_online.assert_called()
        mock_on_device_boot.assert_called()

    @mock.patch.object(
        ffx.FFX,
        "notify_intentional_disconnect",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "on_device_boot",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "wait_for_online",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "wait_for_rcs_disconnection",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_send_reboot_command",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "log_message_to_device",
        autospec=True,
    )
    async def test_reboot(
        self,
        mock_log_message_to_device: mock.Mock,
        mock_send_reboot_command: mock.Mock,
        mock_ffx_wait_for_rcs_disconnection: mock.Mock,
        mock_wait_for_online: mock.Mock,
        mock_on_device_boot: mock.Mock,
        mock_ffx_notify_intentional_disconnect: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.reboot()"""
        mock_process = mock.MagicMock()
        mock_ffx_wait_for_rcs_disconnection.return_value = mock_process

        await self.fd_fc_obj.reboot()

        self.assertEqual(mock_log_message_to_device.call_count, 2)
        mock_send_reboot_command.assert_called()
        mock_ffx_wait_for_rcs_disconnection.assert_called()
        mock_process.wait.assert_called_once_with(timeout=60)
        mock_wait_for_online.assert_called()
        mock_on_device_boot.assert_called()
        mock_ffx_notify_intentional_disconnect.assert_called()

    async def test_suspend_error(self) -> None:
        """Testcase for FuchsiaDevice.suspend() raising NotSupportedError."""
        with self.assertRaises(errors.NotSupportedError):
            await self.fd_fc_obj.suspend()

    @mock.patch.object(
        ffx.FFX,
        "notify_intentional_disconnect",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "boot_id",
        autospec=True,
        return_value="1",
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "before_usb_disconnect",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "wait_for_offline",
        autospec=True,
    )
    async def test_suspend(
        self,
        mock_wait_for_offline: mock.Mock,
        mock_before_usb_disconnect: mock.Mock,
        mock_boot_id: mock.Mock,
        mock_ffx_notify_intentional_disconnect: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.suspend()"""
        mock_usb_power_hub = mock.MagicMock(
            spec=usb_power_hub_interface.UsbPowerHub
        )
        self.fd_fc_obj.set_usb_power_hub(
            usb_power_hub=mock_usb_power_hub, port=1
        )
        await self.fd_fc_obj.suspend()

        mock_boot_id.assert_called()
        mock_before_usb_disconnect.assert_called()
        mock_ffx_notify_intentional_disconnect.assert_called()
        mock_usb_power_hub.power_off.assert_called_with(1)
        mock_wait_for_offline.assert_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "boot_id",
        autospec=True,
        side_effect=["1", "1"],
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "after_usb_reconnect",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "wait_for_online",
        autospec=True,
    )
    async def test_resume(
        self,
        mock_wait_for_online: mock.Mock,
        mock_after_usb_reconnect: mock.Mock,
        mock_boot_id: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.resume()"""
        mock_usb_power_hub = mock.MagicMock(
            spec=usb_power_hub_interface.UsbPowerHub
        )
        self.fd_fc_obj.set_usb_power_hub(
            usb_power_hub=mock_usb_power_hub, port=1
        )
        self.fd_fc_obj._pre_suspend_boot_id = "1"
        await self.fd_fc_obj.resume()

        mock_boot_id.assert_called()
        mock_after_usb_reconnect.assert_called()
        mock_usb_power_hub.power_on.assert_called_with(1)
        mock_wait_for_online.assert_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "boot_id",
        autospec=True,
        return_value="2",
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "after_usb_reconnect",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "wait_for_online",
        autospec=True,
    )
    async def test_resume_error(
        self,
        unused_mock_wait_for_online: mock.Mock,
        unused_mock_after_usb_reconnect: mock.Mock,
        unused_mock_boot_id: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.resume() raising FuchsiaDeviceError."""
        mock_usb_power_hub = mock.MagicMock(
            spec=usb_power_hub_interface.UsbPowerHub
        )
        self.fd_fc_obj.set_usb_power_hub(
            usb_power_hub=mock_usb_power_hub, port=1
        )
        self.fd_fc_obj._pre_suspend_boot_id = "1"
        with self.assertRaises(errors.FuchsiaDeviceError):
            await self.fd_fc_obj.resume()

    def test_register_for_on_device_boot(self) -> None:
        """Testcase for FuchsiaDevice.register_for_on_device_boot()"""
        self.fd_fc_obj.register_for_on_device_boot(fn=lambda: None)

    async def test_register_for_on_device_close(self) -> None:
        """Testcase for FuchsiaDevice.register_for_on_device_close()"""
        self.fd_fc_obj.register_for_on_device_boot(fn=lambda: None)
        await self.fd_fc_obj.close()

    @parameterized.expand(
        [
            (
                {
                    "label": "no_snapshot_file_arg",
                    "directory": "/tmp",
                    "optional_params": {},
                },
            ),
            (
                {
                    "label": "snapshot_file_arg",
                    "directory": "/tmp",
                    "optional_params": {
                        "snapshot_file": "snapshot.zip",
                    },
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "_send_snapshot_command",
        return_value=_BASE64_ENCODED_BYTES,
        autospec=True,
    )
    @mock.patch.object(os, "makedirs", autospec=True)
    async def test_snapshot(
        self,
        parameterized_dict: dict[str, Any],
        mock_makedirs: mock.Mock,
        mock_send_snapshot_command: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.snapshot()"""
        directory: str = parameterized_dict["directory"]
        optional_params: dict[str, Any] = parameterized_dict["optional_params"]

        with mock.patch("builtins.open", mock.mock_open()) as mocked_file:
            snapshot_file_path: str = await self.fd_fc_obj.snapshot(
                directory=directory, **optional_params
            )

        if "snapshot_file" in optional_params:
            self.assertEqual(
                snapshot_file_path,
                f"{directory}/{optional_params['snapshot_file']}",
            )
        else:
            self.assertRegex(
                snapshot_file_path,
                f"{directory}/Snapshot_{self.fd_fc_obj.device_name}_.*.zip",
            )

        mocked_file.assert_called()
        mocked_file().write.assert_called()
        mock_makedirs.assert_called()
        mock_send_snapshot_command.assert_called()

    @mock.patch.object(
        ffx.FFX,
        "wait_for_rcs_disconnection",
        autospec=True,
    )
    def test_wait_for_offline_success(
        self, mock_ffx_wait_for_rcs_disconnection: mock.Mock
    ) -> None:
        """Testcase for FuchsiaDevice.wait_for_offline() success case"""
        self.fd_fc_obj.wait_for_offline()

        mock_ffx_wait_for_rcs_disconnection.assert_called()

    @mock.patch.object(
        ffx.FFX,
        "wait_for_rcs_disconnection",
        side_effect=ffx_errors.FfxCommandError("error"),
        autospec=True,
    )
    def test_wait_for_offline_fail(
        self, mock_ffx_wait_for_rcs_disconnection: mock.Mock
    ) -> None:
        """Testcase for FuchsiaDevice.wait_for_offline() failure case"""
        with self.assertRaisesRegex(
            errors.FuchsiaDeviceError, "failed to go offline"
        ):
            self.fd_fc_obj.wait_for_offline()

        mock_ffx_wait_for_rcs_disconnection.assert_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "resolve_device_ip",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "wait_for_rcs_connection",
        autospec=True,
    )
    async def test_wait_for_online_success_with_ip(
        self,
        mock_ffx_wait_for_rcs_connection: mock.Mock,
        mock_resolve_device_ip: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.wait_for_online() success case when the
        IP address is specified."""
        self.fd_fc_obj._is_static_ip = True
        await self.fd_fc_obj.wait_for_online()

        mock_ffx_wait_for_rcs_connection.assert_called_with(
            mock.ANY, include_target_name=False
        )
        mock_resolve_device_ip.assert_not_called()

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "resolve_device_ip",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "wait_for_rcs_connection",
        autospec=True,
    )
    async def test_wait_for_online_success_no_ip(
        self,
        mock_ffx_wait_for_rcs_connection: mock.Mock,
        mock_resolve_device_ip: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.wait_for_online() success case when the
        IP address is not specified."""
        self.fd_fc_obj._device_info = custom_types.DeviceInfo(
            name=_INPUT_ARGS["device_name"],
            serial_number=None,
            ip_port=None,
            serial_socket=None,
        )
        self.fd_fc_obj._is_static_ip = False
        await self.fd_fc_obj.wait_for_online()

        mock_ffx_wait_for_rcs_connection.assert_called_with(
            mock.ANY, include_target_name=True
        )
        mock_resolve_device_ip.assert_called()

    @mock.patch.object(
        ffx.FFX,
        "wait_for_rcs_connection",
        side_effect=ffx_errors.FfxCommandError("error"),
        autospec=True,
    )
    async def test_wait_for_online_fail(
        self,
        mock_ffx_wait_for_rcs_connection: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.wait_for_online() failure case"""
        self.fd_fc_obj._is_static_ip = True
        with self.assertRaisesRegex(
            errors.FuchsiaDeviceError, "failed to go online"
        ):
            await self.fd_fc_obj.wait_for_online()

        mock_ffx_wait_for_rcs_connection.assert_called_with(
            mock.ANY, include_target_name=False
        )

    # List all the tests related to private properties
    @mock.patch.object(
        f_buildinfo.ProviderClient,
        "get_build_info",
        new_callable=mock.AsyncMock,
        return_value=f_buildinfo.ProviderGetBuildInfoResponse(
            build_info=_MOCK_BUILD_INFO
        ),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_build_info(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_buildinfo_provider: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._build_info property"""
        # pylint: disable=protected-access
        self.assertEqual(await self.fd_fc_obj._build_info(), _MOCK_BUILD_INFO)

        mock_fc_connect_device_proxy.assert_called_once()
        mock_buildinfo_provider.assert_called()

    @mock.patch.object(
        f_buildinfo.ProviderClient,
        "get_build_info",
        new_callable=mock.AsyncMock,
        return_value=f_buildinfo.ProviderGetBuildInfoResponse(
            build_info=_MOCK_BUILD_INFO
        ),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_build_info_error(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_buildinfo_provider: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._build_info property when the get_info
        FIDL call raises an error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        mock_buildinfo_provider.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_INVALID_ARGS
        )
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            # pylint: disable=protected-access
            await self.fd_fc_obj._build_info()

        mock_fc_connect_device_proxy.assert_called_once()

    @mock.patch.object(
        f_hwinfo.DeviceClient,
        "get_info",
        new_callable=mock.AsyncMock,
        return_value=f_hwinfo.DeviceGetInfoResponse(info=_MOCK_DEVICE_INFO),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_device_info_from_fidl(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_hwinfo_device: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._device_info property"""
        # pylint: disable=protected-access
        self.assertEqual(
            await self.fd_fc_obj._device_info_from_fidl(),
            _MOCK_DEVICE_INFO,
        )

        mock_fc_connect_device_proxy.assert_called_once()
        mock_hwinfo_device.assert_called()

    @mock.patch.object(
        f_hwinfo.DeviceClient,
        "get_info",
        new_callable=mock.AsyncMock,
        return_value=f_hwinfo.DeviceGetInfoResponse(info=_MOCK_DEVICE_INFO),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_device_info_from_fidl_error(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_hwinfo_device: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._device_info property when the get_info
        FIDL call raises an error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        mock_hwinfo_device.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_INVALID_ARGS
        )
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            # pylint: disable=protected-access
            await self.fd_fc_obj._device_info_from_fidl()

        mock_fc_connect_device_proxy.assert_called_once()

    @mock.patch.object(
        f_hwinfo.ProductClient,
        "get_info",
        new_callable=mock.AsyncMock,
        return_value=f_hwinfo.ProductGetInfoResponse(info=_MOCK_PRODUCT_INFO),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_product_info(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_hwinfo_product: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._product_info property"""
        # pylint: disable=protected-access
        self.assertEqual(
            await self.fd_fc_obj._product_info(),
            _MOCK_PRODUCT_INFO,
        )

        mock_fc_connect_device_proxy.assert_called_once()
        mock_hwinfo_product.assert_called()

    @mock.patch.object(
        f_hwinfo.ProductClient,
        "get_info",
        new_callable=mock.AsyncMock,
        return_value=f_hwinfo.ProductGetInfoResponse(info=_MOCK_PRODUCT_INFO),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_product_info_error(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_hwinfo_product: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._product_info property when the get_info
        FIDL call raises an error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        mock_hwinfo_product.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_INVALID_ARGS
        )
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            # pylint: disable=protected-access
            await self.fd_fc_obj._product_info()

        mock_fc_connect_device_proxy.assert_called_once()

    # List all the tests related to private methods
    @parameterized.expand(
        [
            (
                {
                    "label": "info_level",
                    "log_level": custom_types.LEVEL.INFO,
                    "log_message": "info message",
                },
            ),
            (
                {
                    "label": "warning_level",
                    "log_level": custom_types.LEVEL.WARNING,
                    "log_message": "warning message",
                },
            ),
            (
                {
                    "label": "error_level",
                    "log_level": custom_types.LEVEL.ERROR,
                    "log_message": "error message",
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        fd_remotecontrol.RemoteControlClient,
        "log_message",
        new_callable=mock.AsyncMock,
    )
    async def test_send_log_command(
        self,
        parameterized_dict: dict[str, Any],
        mock_rcs_log_message: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._send_log_command()"""
        self.fd_fc_obj.fuchsia_controller.ctx = mock.Mock()
        # pylint: disable=protected-access
        await self.fd_fc_obj._send_log_command(
            tag="test",
            level=parameterized_dict["log_level"],
            message=parameterized_dict["log_message"],
        )

        mock_rcs_log_message.assert_called()

    @mock.patch.object(
        fd_remotecontrol.RemoteControlClient,
        "log_message",
        new_callable=mock.AsyncMock,
    )
    async def test_send_log_command_error(
        self, mock_rcs_log_message: mock.Mock
    ) -> None:
        """Testcase for FuchsiaDevice._send_log_command() when the log FIDL call
        raises an error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        self.fd_fc_obj.fuchsia_controller.ctx = mock.Mock()

        mock_rcs_log_message.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_INVALID_ARGS
        )
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            # pylint: disable=protected-access
            await self.fd_fc_obj._send_log_command(
                tag="test", level=custom_types.LEVEL.ERROR, message="test"
            )

    @mock.patch.object(
        fhp_statecontrol.AdminClient,
        "shutdown",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_send_reboot_command(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_admin_shutdown: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._send_reboot_command()"""
        # pylint: disable=protected-access
        await self.fd_fc_obj._send_reboot_command()

        mock_fc_connect_device_proxy.assert_called()
        mock_admin_shutdown.assert_called()

    @mock.patch.object(
        fhp_statecontrol.AdminClient,
        "shutdown",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_send_reboot_command_error(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_admin_shutdown: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._send_reboot_command() when the reboot
        FIDL call raises a non-FC_ERR_FDOMAIN error.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        mock_admin_shutdown.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_INVALID_ARGS
        )
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            # pylint: disable=protected-access
            await self.fd_fc_obj._send_reboot_command()

        mock_fc_connect_device_proxy.assert_called()
        mock_admin_shutdown.assert_called()

    @mock.patch.object(
        fhp_statecontrol.AdminClient,
        "shutdown",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    async def test_send_reboot_command_error_is_peer_closed(
        self,
        mock_fc_connect_device_proxy: mock.Mock,
        mock_admin_shutdown: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice._send_reboot_command() when the reboot
        FIDL call raises a FC_ERR_FDOMAIN error.  This error should not
        result in `FuchsiaControllerError` being raised."""
        mock_admin_shutdown.side_effect = FcTransportStatus(
            FcTransportStatus.FC_ERR_FDOMAIN
        )
        # pylint: disable=protected-access
        await self.fd_fc_obj._send_reboot_command()

        mock_fc_connect_device_proxy.assert_called()
        mock_admin_shutdown.assert_called()

    @mock.patch.object(
        f_feedback.DataProviderClient,
        "get_snapshot",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_io.FileClient,
        "get_attributes",
        new_callable=mock.AsyncMock,
        return_value=_file_attr_resp(ZxStatus(ZxStatus.ZX_OK), 15),
    )
    @mock.patch.object(
        f_io.FileClient,
        "read",
        new_callable=mock.AsyncMock,
        side_effect=[
            # Read 15 bytes over multiple responses.
            _file_read_result([0] * 5),
            _file_read_result([0] * 5),
            _file_read_result([0] * 5),
            # Send empty response to signal read completion.
            _file_read_result([]),
        ],
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    async def test_send_snapshot_command(
        self,
        unused_mock_fc_create_context: mock.Mock,
        unused_mock_health_check: mock.Mock,
        mock_fc_connect_device_proxy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Testcase for FuchsiaDevice._send_snapshot_command()"""
        # pylint: disable=protected-access
        data = await self.fd_fc_obj._send_snapshot_command()
        self.assertEqual(len(data), 15)

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        f_feedback.DataProviderClient,
        "get_snapshot",
        new_callable=mock.AsyncMock,
        # Raise arbitrary failure.
        side_effect=FcTransportStatus(FcTransportStatus.FC_ERR_INVALID_ARGS),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    async def test_send_snapshot_command_get_snapshot_error(
        self,
        unused_mock_fc_create_context: mock.Mock,
        unused_mock_health_check: mock.Mock,
        mock_fc_connect_device_proxy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Testcase for FuchsiaDevice._send_snapshot_command() when the
        get_snapshot FIDL call raises an exception.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        # pylint: disable=protected-access
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            await self.fd_fc_obj._send_snapshot_command()

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        f_feedback.DataProviderClient,
        "get_snapshot",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_io.FileClient,
        "get_attributes",
        new_callable=mock.AsyncMock,
        # Raise arbitrary failure.
        side_effect=ZxStatus(ZxStatus.ZX_ERR_INVALID_ARGS),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    async def test_send_snapshot_command_get_attributes_error(
        self,
        unused_mock_fc_create_context: mock.Mock,
        unused_mock_health_check: mock.Mock,
        mock_fc_connect_device_proxy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Testcase for FuchsiaDevice._send_snapshot_command() when the get_attributes
        FIDL call raises an exception.
        FC_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        # pylint: disable=protected-access
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            await self.fd_fc_obj._send_snapshot_command()

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        f_feedback.DataProviderClient,
        "get_snapshot",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_io.FileClient,
        "get_attributes",
        new_callable=mock.AsyncMock,
        return_value=_file_attr_resp(ZxStatus(ZxStatus.ZX_ERR_INVALID_ARGS), 0),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    async def test_send_snapshot_command_get_attributes_status_not_ok(
        self,
        unused_mock_fc_create_context: mock.Mock,
        unused_mock_health_check: mock.Mock,
        mock_fc_connect_device_proxy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Testcase for FuchsiaDevice._send_snapshot_command() when the get_attributes
        FIDL call returns a non-OK status code.
        ZX_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        # pylint: disable=protected-access
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            await self.fd_fc_obj._send_snapshot_command()

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        f_feedback.DataProviderClient,
        "get_snapshot",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_io.FileClient,
        "get_attributes",
        new_callable=mock.AsyncMock,
        return_value=_file_attr_resp(ZxStatus(ZxStatus.ZX_OK), 15),
    )
    @mock.patch.object(
        f_io.FileClient,
        "read",
        new_callable=mock.AsyncMock,
        side_effect=ZxStatus(ZxStatus.ZX_ERR_INVALID_ARGS),
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    async def test_send_snapshot_command_read_error(
        self,
        unused_mock_fc_create_context: mock.Mock,
        unused_mock_health_check: mock.Mock,
        mock_fc_connect_device_proxy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Testcase for FuchsiaDevice._send_snapshot_command() when the read
        FIDL call raises an exception.
        ZX_ERR_INVALID_ARGS was chosen arbitrarily for this purpose."""
        # pylint: disable=protected-access
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            await self.fd_fc_obj._send_snapshot_command()

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        f_feedback.DataProviderClient,
        "get_snapshot",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_io.FileClient,
        "get_attributes",
        new_callable=mock.AsyncMock,
        # File reports size of 15 bytes.
        return_value=_file_attr_resp(ZxStatus(ZxStatus.ZX_OK), 15),
    )
    @mock.patch.object(
        f_io.FileClient,
        "read",
        new_callable=mock.AsyncMock,
        # Only 5 bytes are read.
        side_effect=[
            _file_read_result([0] * 5),
            _file_read_result([]),
        ],
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "connect_device_proxy",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "create_context",
        autospec=True,
    )
    async def test_send_snapshot_command_size_mismatch(
        self,
        unused_mock_fc_create_context: mock.Mock,
        unused_mock_health_check: mock.Mock,
        mock_fc_connect_device_proxy: mock.Mock,
        *unused_args: Any,
    ) -> None:
        """Testcase for FuchsiaDevice._send_snapshot_command() when the number
        of bytes read from channel doesn't match the file's content size."""
        # pylint: disable=protected-access
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            await self.fd_fc_obj._send_snapshot_command()

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value="core/starnix_runner/kernels:",
        autospec=True,
    )
    def test_is_starnix_device(self, mock_ffx: mock.Mock) -> None:
        """Testcase for FuchsiaDevice.is_starnix_device()"""
        self.assertTrue(self.fd_fc_obj.is_starnix_device())

        mock_ffx.assert_called_once()

    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value="",
        autospec=True,
    )
    def test_is_starnix_device_unsupported_error(
        self, mock_ffx: mock.Mock
    ) -> None:
        """Testcase for FuchsiaDevice.is_starnix_device()"""
        self.assertFalse(self.fd_fc_obj.is_starnix_device())

        mock_ffx.assert_called_once()

    @mock.patch.object(
        ffx.FFX,
        "run",
        side_effect=ffx_errors.FfxCommandError("error"),
        autospec=True,
    )
    def test_is_starnix_device_error(self, mock_ffx: mock.Mock) -> None:
        """Testcase for FuchsiaDevice.is_starnix_device()"""
        with self.assertRaises(errors.FuchsiaDeviceError):
            self.fd_fc_obj.is_starnix_device()

        mock_ffx.assert_called_once()

    def test_register_for_on_device_ip_change(self) -> None:
        """Testcase for FuchsiaDevice.register_for_on_device_ip_change()"""
        self.fd_fc_obj.register_for_on_device_ip_change(fn=lambda x: None)

    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value=_MOCK_ARGS["ffx_target_ssh_address_output"],
        autospec=True,
    )
    async def test_resolve_device_ip(
        self,
        mock_ffx_run: mock.Mock,
        mock_fuchsia_device_health_check: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.resolve_device_ip()"""
        with mock.patch.object(self.fd_fc_obj, "_on_device_ip_change_fns"):
            await self.fd_fc_obj.resolve_device_ip()
        mock_ffx_run.assert_called_once_with(
            self.fd_fc_obj.ffx,
            cmd=ffx._FFX_CMDS["TARGET_SSH_ADDRESS"]
            + [self.fd_fc_obj.device_name],
            include_target=False,
        )
        mock_fuchsia_device_health_check.assert_called_once()

    # Verifies FuchsiaDevice.resolve_device_ip() re-resolves target address for USB-connected devices.
    @mock.patch.object(
        fuchsia_device.FuchsiaDevice,
        "health_check",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "resolve_target_address",
        autospec=True,
    )
    @mock.patch.object(
        fc_transport.FuchsiaController,
        "after_usb_reconnect",
        autospec=True,
    )
    @mock.patch.object(
        ffx.FFX,
        "run",
        return_value=json.dumps(
            [
                {
                    "nodename": "fuchsia-emulator",
                    "rcs_state": "Y",
                    "serial": "<unknown>",
                    "target_type": "core.arm64",
                    "target_state": "Product",
                    "addresses": [{"cid": 13}],
                    "is_default": False,
                    "is_manual": False,
                }
            ]
        ),
        autospec=True,
    )
    async def test_resolve_device_ip_usb(
        self,
        mock_ffx_run: mock.Mock,
        mock_fc_after_usb_reconnect: mock.Mock,
        mock_ffx_resolve_target_address: mock.Mock,
        mock_fuchsia_device_health_check: mock.Mock,
    ) -> None:
        """Testcase for FuchsiaDevice.resolve_device_ip() with USB target."""
        await self.fd_fc_obj.resolve_device_ip()
        mock_ffx_run.assert_called_once_with(
            self.fd_fc_obj.ffx,
            cmd=fuchsia_device._FFX_CMDS["RESOLVE_IP"]
            + [self.fd_fc_obj.device_name],
            include_target=False,
        )
        mock_ffx_resolve_target_address.assert_called_once()
        mock_fc_after_usb_reconnect.assert_called_once()
        mock_fuchsia_device_health_check.assert_called_once()


if __name__ == "__main__":
    unittest.main()
