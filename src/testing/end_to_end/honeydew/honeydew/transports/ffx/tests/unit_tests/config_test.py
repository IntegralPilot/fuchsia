# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for ffx.config.py."""

import json
import os
import unittest
from typing import Any
from unittest import mock

import fuchsia_controller_py as fuchsia_controller
from honeydew.transports.ffx import config as ffx_config
from honeydew.utils import host_shell

# pylint: disable=protected-access
_TARGET_NAME: str = "fuchsia-emulator"

_ISOLATE_DIR: str = "/tmp/isolate"
_LOGS_DIR: str = "/tmp/logs"
_BINARY_PATH: str = "ffx"
_LOGS_LEVEL: str = "debug"
_MDNS_ENABLED: bool = False
_ENABLE_USB: bool = False
_USB_SOCKET_PATH: str | None = None
_USB_DRIVER_AUTOSTART: bool = False
_SUBTOOLS_SEARCH_PATH: str = "/subtools"
_PROXY_TIMEOUT_SECS: int = 30
_SSH_KEEPALIVE_TIMEOUT: int = 60

_FFX_CMD_OPTIONS: list[str] = [
    "ffx",
    "--isolate-dir",
    _ISOLATE_DIR,
]

_INPUT_ARGS: dict[str, Any] = {
    "target_name": _TARGET_NAME,
    "ffx_config_data": ffx_config.FfxConfigData(
        isolate_dir=fuchsia_controller.IsolateDir(_ISOLATE_DIR),
        logs_dir=_LOGS_DIR,
        binary_path=_BINARY_PATH,
        logs_level=_LOGS_LEVEL,
        enable_usb=_ENABLE_USB,
        usb_socket_path=_USB_SOCKET_PATH,
        usb_driver_autostart=_USB_DRIVER_AUTOSTART,
        subtools_search_path=_SUBTOOLS_SEARCH_PATH,
        proxy_timeout_secs=_PROXY_TIMEOUT_SECS,
        ssh_keepalive_timeout=_SSH_KEEPALIVE_TIMEOUT,
        emu_instance_dir=None,
        ssh_private_keys=[],
        ssh_public_keys=[],
        ssh_auth_sock=None,
        identities_only=None,
    ),
}


class FfxConfigTests(unittest.TestCase):
    """Unit tests for ffx.config.FfxConfig"""

    @mock.patch.object(
        host_shell,
        "run",
        autospec=True,
    )
    def test_setup(self, mock_host_shell_run: mock.Mock) -> None:
        """Test case for FfxConfig.setup()"""

        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()

        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            usb_socket_path=_USB_SOCKET_PATH,
            usb_driver_autostart=_USB_DRIVER_AUTOSTART,
            subtools_search_path=_SUBTOOLS_SEARCH_PATH,
            proxy_timeout_secs=_PROXY_TIMEOUT_SECS,
            ssh_keepalive_timeout=_SSH_KEEPALIVE_TIMEOUT,
        )

        mock_host_shell_run.assert_not_called()

        # Calling setup() again should fail
        with self.assertRaises(ffx_config.FfxConfigError):
            ffx_config_obj.setup(
                binary_path=_BINARY_PATH,
                isolate_dir=_ISOLATE_DIR,
                logs_dir=_LOGS_DIR,
                logs_level=_LOGS_LEVEL,
                enable_mdns=_MDNS_ENABLED,
                enable_usb=_ENABLE_USB,
                usb_socket_path=_USB_SOCKET_PATH,
                usb_driver_autostart=_USB_DRIVER_AUTOSTART,
                subtools_search_path=_SUBTOOLS_SEARCH_PATH,
                proxy_timeout_secs=_PROXY_TIMEOUT_SECS,
                ssh_keepalive_timeout=_SSH_KEEPALIVE_TIMEOUT,
            )

    def test_close(self) -> None:
        """Test case for ffx_config.FfxConfig.close()"""

        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()

        # Call setup first before calling close
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            usb_socket_path=_USB_SOCKET_PATH,
            usb_driver_autostart=_USB_DRIVER_AUTOSTART,
            subtools_search_path=_SUBTOOLS_SEARCH_PATH,
            proxy_timeout_secs=_PROXY_TIMEOUT_SECS,
            ssh_keepalive_timeout=_SSH_KEEPALIVE_TIMEOUT,
        )

        ffx_config_obj.close()

    def test_close_without_setup(self) -> None:
        """Test case for ffx_config.FfxConfig.close() without calling
        ffx_config.FfxConfig.setup()"""

        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()

        # Calling setup() again should fail
        with self.assertRaises(ffx_config.FfxConfigError):
            ffx_config_obj.close()

    @mock.patch("honeydew.transports.ffx.config.os.environ", {}, autospec=False)
    @mock.patch(
        "honeydew.transports.ffx.config.os.path.exists",
        return_value=False,
        autospec=True,
    )
    def test_get_config(self, unused_mock_exists: mock.Mock) -> None:
        """Test case for ffx_config.FfxConfig.get_config()"""

        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()

        # Call setup first before calling close
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            usb_socket_path=_USB_SOCKET_PATH,
            usb_driver_autostart=_USB_DRIVER_AUTOSTART,
            subtools_search_path=_SUBTOOLS_SEARCH_PATH,
            proxy_timeout_secs=_PROXY_TIMEOUT_SECS,
            ssh_keepalive_timeout=_SSH_KEEPALIVE_TIMEOUT,
        )

        self.assertEqual(
            str(ffx_config_obj.get_config()),
            str(_INPUT_ARGS["ffx_config_data"]),
        )

    def test_get_config_without_setup(self) -> None:
        """Test case for ffx_config.FfxConfig.get_config() without calling
        ffx_config.FfxConfig.setup()"""

        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()

        # Calling setup() again should fail
        with self.assertRaises(ffx_config.FfxConfigError):
            ffx_config_obj.get_config()

    @mock.patch("honeydew.transports.ffx.config.os.path.exists", autospec=True)
    def test_setup_with_ssh_keys_fallback(self, mock_exists: mock.Mock) -> None:
        """Test case for FfxConfig.setup() with SSH keys fallback"""
        mock_exists.side_effect = lambda p: p == "/path/to/key1.pub"

        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            ssh_private_keys=["/path/to/key1", "/path/to/key2"],
        )
        config = ffx_config_obj.get_config()
        self.assertEqual(
            config.ssh_private_keys, ["/path/to/key1", "/path/to/key2"]
        )
        # /path/to/key2.pub does not exist (mocked), so only key1.pub should be here
        self.assertEqual(config.ssh_public_keys, ["/path/to/key1.pub"])

    def test_setup_with_explicit_ssh_keys(self) -> None:
        """Test case for FfxConfig.setup() with explicit SSH keys"""
        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            ssh_private_keys=["/path/to/key1"],
            ssh_public_keys=["/path/to/pub1", "/path/to/pub2"],
        )
        config = ffx_config_obj.get_config()
        self.assertEqual(config.ssh_private_keys, ["/path/to/key1"])
        self.assertEqual(
            config.ssh_public_keys, ["/path/to/pub1", "/path/to/pub2"]
        )

    def test_setup_with_explicit_ssh_auth_sock(self) -> None:
        """Test case for FfxConfig.setup() with explicit ssh_auth_sock"""
        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            ssh_auth_sock="/tmp/custom_auth_sock",
        )
        config = ffx_config_obj.get_config()
        self.assertEqual(config.ssh_auth_sock, "/tmp/custom_auth_sock")

    @mock.patch.dict(
        "honeydew.transports.ffx.config.os.environ",
        {"SSH_AUTH_SOCK": "/tmp/env_auth_sock"},
    )
    def test_setup_with_env_ssh_auth_sock(self) -> None:
        """Test case for FfxConfig.setup() with SSH_AUTH_SOCK from environment"""
        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
        )
        config = ffx_config_obj.get_config()
        self.assertEqual(config.ssh_auth_sock, "/tmp/env_auth_sock")

    def test_setup_with_identities_only(self) -> None:
        """Test case for FfxConfig.setup(identities_only=True)"""
        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
            identities_only=True,
        )
        config = ffx_config_obj.get_config()
        self.assertTrue(config.identities_only)

    @mock.patch.dict(os.environ, {"IDENTITIES_ONLY": "yes"}, clear=True)
    def test_setup_with_identities_only_from_env(self) -> None:
        """Test case for FfxConfig.setup() reading IDENTITIES_ONLY from env"""
        ffx_config_obj: ffx_config.FfxConfig = ffx_config.FfxConfig()
        ffx_config_obj.setup(
            binary_path=_BINARY_PATH,
            isolate_dir=_ISOLATE_DIR,
            logs_dir=_LOGS_DIR,
            logs_level=_LOGS_LEVEL,
            enable_mdns=_MDNS_ENABLED,
            enable_usb=_ENABLE_USB,
        )
        config = ffx_config_obj.get_config()
        self.assertTrue(config.identities_only)

    def test_get_config_args(self) -> None:
        """Test case for FfxConfigData.get_config_args()"""
        ffx_config_data = ffx_config.FfxConfigData(
            isolate_dir=fuchsia_controller.IsolateDir(_ISOLATE_DIR),
            logs_dir=_LOGS_DIR,
            binary_path=_BINARY_PATH,
            logs_level=_LOGS_LEVEL,
            enable_usb=_ENABLE_USB,
            usb_socket_path=_USB_SOCKET_PATH,
            usb_driver_autostart=_USB_DRIVER_AUTOSTART,
            subtools_search_path=_SUBTOOLS_SEARCH_PATH,
            proxy_timeout_secs=_PROXY_TIMEOUT_SECS,
            ssh_keepalive_timeout=_SSH_KEEPALIVE_TIMEOUT,
            emu_instance_dir=None,
            ssh_private_keys=[],
            ssh_public_keys=[],
            ssh_auth_sock="/tmp/ssh_auth_sock",
            identities_only=True,
        )
        expected_config_dict = {
            "log": {"dir": _LOGS_DIR, "level": _LOGS_LEVEL},
            "ffx": {"subtool-search-paths": [_SUBTOOLS_SEARCH_PATH]},
            "proxy": {"timeout_secs": _PROXY_TIMEOUT_SECS},
            "ssh": {
                "keepalive_timeout": _SSH_KEEPALIVE_TIMEOUT,
                "priv": [],
                "pub": [],
                "auth-sock": "/tmp/ssh_auth_sock",
                "identities-only": True,
            },
            "connectivity": {
                "enable_usb": _ENABLE_USB,
                "usb_driver_autostart": _USB_DRIVER_AUTOSTART,
            },
        }
        self.assertEqual(
            ffx_config_data.get_config_args(),
            ["-c", json.dumps(expected_config_dict)],
        )
