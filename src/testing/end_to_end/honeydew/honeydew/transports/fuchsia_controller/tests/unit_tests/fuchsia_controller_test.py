# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.transports.fuchsia_controller.py."""

import ipaddress
import unittest
from typing import Any
from unittest import mock

import fuchsia_controller_py as fuchsia_controller
from honeydew import affordances_capable
from honeydew.transports.ffx import config as ffx_config
from honeydew.transports.fuchsia_controller import errors as fc_errors
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types

_TARGET_NAME: str = "fuchsia-emulator"

_IPV6: str = "fe80::4fce:3102:ef13:888c%1"
_IPV6_OBJ: ipaddress.IPv6Address = ipaddress.IPv6Address(_IPV6)

_SSH_ADDRESS: ipaddress.IPv6Address = _IPV6_OBJ
_SSH_PORT = 8022
_TARGET_IP_PORT = custom_types.IpPort(ip=_SSH_ADDRESS, port=_SSH_PORT)


_INPUT_ARGS: dict[str, Any] = {
    "target_name": _TARGET_NAME,
    "target_ip_port": _TARGET_IP_PORT,
    "BuildInfo": custom_types.FidlEndpoint(
        "/core/build-info", "fuchsia.buildinfo.Provider"
    ),
}

_MOCK_ARGS: dict[str, Any] = {
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


# pylint: disable=protected-access
class FuchsiaControllerTests(unittest.TestCase):
    """Unit tests for honeydew.transports.fuchsia_controller.py."""

    def setUp(self) -> None:
        super().setUp()

        with mock.patch.object(
            fuchsia_controller.Context,
            "target_wait",
            autospec=True,
        ) as mock_target_wait:
            self.fuchsia_controller_obj_wo_device_ip = (
                fc_transport.FuchsiaController(
                    target_name=_INPUT_ARGS["target_name"],
                    ffx_config_data=_MOCK_ARGS["ffx_config_data"],
                )
            )
        mock_target_wait.assert_called()

        mock_target_wait.reset_mock()

        with (
            mock.patch.object(
                fuchsia_controller.Context,
                "target_wait",
                autospec=True,
            ) as mock_target_wait,
        ):
            self.device_ip_change = mock.MagicMock(
                spec=affordances_capable.FuchsiaDeviceIpChange
            )

            self.fuchsia_controller_obj_with_device_ip = (
                fc_transport.FuchsiaController(
                    target_name=_INPUT_ARGS["target_name"],
                    target_ip_port=_INPUT_ARGS["target_ip_port"],
                    ffx_config_data=_MOCK_ARGS["ffx_config_data"],
                    device_ip_change=self.device_ip_change,
                )
            )
        mock_target_wait.assert_called()

    def test_create_context(self) -> None:
        """Test case for fuchsia_controller_transport.create_context()."""
        self.fuchsia_controller_obj_with_device_ip.create_context()

    @mock.patch.object(
        fuchsia_controller,
        "Context",
        autospec=True,
    )
    def test_create_context_with_ssh_auth_sock(
        self, mock_fc_context: mock.Mock
    ) -> None:
        """Test case for fuchsia_controller_transport.create_context() with ssh_auth_sock."""
        ssh_auth_sock = "/tmp/ssh_auth_sock"
        ffx_config_data = ffx_config.FfxConfigData(
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
            ssh_auth_sock=ssh_auth_sock,
        )
        fc_transport.FuchsiaController(
            target_name=_INPUT_ARGS["target_name"],
            ffx_config_data=ffx_config_data,
        )

        mock_fc_context.assert_called_with(
            config={
                "log.level": "debug",
                "log.dir": "/tmp/logs",
                "connectivity.enable_usb": "false",
                "connectivity.usb_driver_autostart": "false",
                "ssh.auth-sock": ssh_auth_sock,
            },
            isolate_dir=ffx_config_data.isolate_dir,
            target=_INPUT_ARGS["target_name"],
        )

    @mock.patch.object(
        fuchsia_controller.Context,
        "target_wait",
        autospec=True,
    )
    @mock.patch.object(
        fuchsia_controller,
        "Context",
        autospec=True,
    )
    def test_create_context_with_identities_only(
        self,
        mock_fc_context: mock.Mock,
        mock_target_wait: mock.Mock,
    ) -> None:
        """Verify create_context() with identities_only passed in ffx_config_data."""
        ffx_config_data = ffx_config.FfxConfigData(
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
            identities_only=True,
        )
        fc_transport.FuchsiaController(
            target_name=_INPUT_ARGS["target_name"],
            ffx_config_data=ffx_config_data,
        )

        mock_fc_context.assert_called_with(
            config={
                "log.level": "debug",
                "log.dir": "/tmp/logs",
                "connectivity.enable_usb": "false",
                "connectivity.usb_driver_autostart": "false",
                "ssh.identities-only": "true",
            },
            isolate_dir=ffx_config_data.isolate_dir,
            target=_INPUT_ARGS["target_name"],
        )

    @mock.patch.object(
        fuchsia_controller,
        "Context",
        side_effect=fuchsia_controller.FcTransportStatus(
            fuchsia_controller.FcTransportStatus.FC_ERR_INVALID_ARGS
        ),
        autospec=True,
    )
    def test_create_context_creation_error(
        self, mock_fc_context: mock.Mock
    ) -> None:
        """Verify create_context() when the fuchsia controller Context creation
        raises an error."""
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            self.fuchsia_controller_obj_with_device_ip.create_context()

        mock_fc_context.assert_called()

    @mock.patch.object(
        fuchsia_controller.Context,
        "connect_device_proxy",
        autospec=True,
    )
    def test_connect_device_proxy(
        self, mock_fc_connect_device_proxy: mock.Mock
    ) -> None:
        """Test case for fuchsia_controller_transport.connect_device_proxy()"""
        self.fuchsia_controller_obj_with_device_ip.connect_device_proxy(
            _INPUT_ARGS["BuildInfo"]
        )

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        fuchsia_controller.Context,
        "connect_device_proxy",
        side_effect=fuchsia_controller.FcTransportStatus(
            fuchsia_controller.FcTransportStatus.FC_ERR_INVALID_ARGS
        ),
        autospec=True,
    )
    def test_connect_device_proxy_error(
        self, mock_fc_connect_device_proxy: mock.Mock
    ) -> None:
        """Test case for fuchsia_controller_transport.connect_device_proxy()"""
        with self.assertRaises(fc_errors.FuchsiaControllerError):
            self.fuchsia_controller_obj_with_device_ip.connect_device_proxy(
                _INPUT_ARGS["BuildInfo"]
            )

        mock_fc_connect_device_proxy.assert_called()

    @mock.patch.object(
        fuchsia_controller.Context,
        "target_wait",
        autospec=True,
    )
    def test_check_connection(self, mock_target_wait: mock.Mock) -> None:
        """Testcase for FuchsiaController.check_connection()"""
        self.fuchsia_controller_obj_with_device_ip.check_connection()

        mock_target_wait.assert_called()

    @mock.patch.object(
        fuchsia_controller.Context,
        "target_wait",
        side_effect=RuntimeError("error"),
        autospec=True,
    )
    def test_check_connection_raises(self, mock_target_wait: mock.Mock) -> None:
        """Testcase for FuchsiaController.check_connection() raises
        FuchsiaControllerConnectionError"""
        with self.assertRaises(fc_errors.FuchsiaControllerConnectionError):
            self.fuchsia_controller_obj_with_device_ip.check_connection()

        mock_target_wait.assert_called()
