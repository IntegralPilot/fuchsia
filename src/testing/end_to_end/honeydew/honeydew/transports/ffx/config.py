# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Provides methods to configure FFX."""

import atexit
import json
import logging
import os
from dataclasses import dataclass
from typing import Any

import fuchsia_controller_py as fuchsia_controller
from honeydew import errors

_FFX_BINARY: str = "ffx"

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FfxConfigError(errors.HoneydewError):
    """Raised by FfxConfig class."""


@dataclass(frozen=True)
class FfxConfigData:
    """Dataclass that holds FFX config information.

    Args:
        binary_path: absolute path to the FFX binary.
        isolate_dir: Directory that will be passed to `--isolate-dir`
            arg of FFX
        logs_dir: Directory that will be passed to `--config log.dir`
            arg of FFX
        logs_level: logs level that will be passed to `--config log.level`
            arg of FFX
        enable_mdns: Whether or not mdns need to be enabled. This will be
            passed to `--config discovery.mdns.enabled` arg of FFX
        subtools_search_path: A path of where ffx should
            look for plugins.
        proxy_timeout_secs: Proxy timeout in secs.
        ssh_keepalive_timeout: SSH keep-alive timeout in secs.
        enable_usb: Whether to use FFX's USB protocol to communicate
            with targets if available.
        usb_socket_path: Path to socket used to communicate with the USB
            protocol driver.
        usb_driver_autostart: Whether to start the USB protocol driver if it
            isn't running.
        emu_instance_dir: Directory where emulators are stored.
        ssh_private_keys: List of SSH private keys for connection.
        ssh_public_keys: List of SSH public keys for connection.
        ssh_auth_sock: Path to SSH authentication socket for connection.
        identities_only: Whether to strictly use explicit identity files.
    """

    binary_path: str
    isolate_dir: fuchsia_controller.IsolateDir
    logs_dir: str
    logs_level: str | None
    subtools_search_path: str | None
    proxy_timeout_secs: int | None
    ssh_keepalive_timeout: int | None
    enable_usb: bool
    usb_socket_path: str | None
    usb_driver_autostart: bool
    emu_instance_dir: str | None
    ssh_private_keys: list[str] | None
    ssh_public_keys: list[str] | None
    ssh_auth_sock: str | None = None
    identities_only: bool | None = None

    def __str__(self) -> str:
        return (
            f"binary_path={self.binary_path}, "
            f"isolate_dir={self.isolate_dir.directory()}, "
            f"logs_dir={self.logs_dir}, "
            f"logs_level={self.logs_level}, "
            f"subtools_search_path={self.subtools_search_path}, "
            f"proxy_timeout_secs={self.proxy_timeout_secs}, "
            f"ssh_keepalive_timeout={self.ssh_keepalive_timeout}, "
            f"enable_usb={self.enable_usb}, "
            f"usb_socket_path={self.usb_socket_path}, "
            f"usb_driver_autostart={self.usb_driver_autostart}, "
            f"emu_instance_dir={self.emu_instance_dir}, "
            f"ssh_private_keys={self.ssh_private_keys}, "
            f"ssh_public_keys={self.ssh_public_keys}, "
            f"ssh_auth_sock={self.ssh_auth_sock}, "
            f"identities_only={self.identities_only}, "
        )

    def get_config_args(self) -> list[str]:
        """Returns the FFX command arguments to set the configuration.

        Returns:
            List of FFX command arguments.
        """

        def set_nested(d: dict[str, Any], key_path: str, value: Any) -> None:
            keys = key_path.split(".")
            for k in keys[:-1]:
                if k not in d:
                    d[k] = {}
                d = d[k]
            d[keys[-1]] = value

        config_dict: dict[str, Any] = {}

        # Map of hierarchical ffx config key to value.
        # Values can be None, which will be ignored.
        configs_to_set: dict[str, Any] = {
            "log.dir": self.logs_dir,
            "log.level": self.logs_level,
            "ffx.subtool-search-paths": (
                [self.subtools_search_path]
                if self.subtools_search_path is not None
                else None
            ),
            "proxy.timeout_secs": self.proxy_timeout_secs,
            "ssh.keepalive_timeout": self.ssh_keepalive_timeout,
            "connectivity.enable_usb": self.enable_usb,
            "connectivity.usb_driver_autostart": self.usb_driver_autostart,
            "connectivity.usb_socket_path": self.usb_socket_path,
            "emu.instance_dir": self.emu_instance_dir,
            "ssh.priv": self.ssh_private_keys,
            "ssh.pub": self.ssh_public_keys,
            "ssh.auth-sock": self.ssh_auth_sock,
            "ssh.identities-only": self.identities_only,
        }

        for key_path, value in configs_to_set.items():
            if value is not None:
                set_nested(config_dict, key_path, value)

        return ["-c", json.dumps(config_dict)]


class FfxConfig:
    """Provides methods to configure FFX."""

    def __init__(self) -> None:
        self._setup_done: bool = False

    def setup(
        self,
        binary_path: str | None,
        isolate_dir: str | None,
        logs_dir: str,
        logs_level: str | None,
        enable_mdns: bool,  # pylint: disable=unused-argument
        enable_usb: bool,
        subtools_search_path: str | None = None,
        proxy_timeout_secs: int | None = None,
        ssh_keepalive_timeout: int | None = None,
        usb_socket_path: str | None = None,
        usb_driver_autostart: bool = True,
        emu_instance_dir: str | None = None,
        ssh_private_keys: list[str] | None = None,
        ssh_public_keys: list[str] | None = None,
        ssh_auth_sock: str | None = None,
        identities_only: bool | None = None,
    ) -> None:
        """Sets up configuration need to be used while running FFX command.

        Args:
            binary_path: absolute path to the FFX binary.
            isolate_dir: Directory that will be passed to `--isolate-dir`
                arg of FFX. If set to None, a random directory will be created.
            logs_dir: Directory that will be passed to `--config log.dir`
                arg of FFX
            logs_level: logs level that will be passed to `--config log.level`
                arg of FFX
            enable_mdns: Whether or not mdns need to be enabled. This will be
                passed to `--config discovery.mdns.enabled` arg of FFX
            enable_usb: Whether to use FFX's USB protocol to communicate.
            subtools_search_path: A path of where ffx should look for plugins.
                Default value is None which means, it will not update
                proxy_timeout_secs
            proxy_timeout_secs: Proxy timeout in secs. Default value is None
                which means, it will not update proxy_timeout_secs
            ssh_keepalive_timeout: SSH keep-alive timeout in secs.
                Default value is None which means, it will not update
                ssh_keepalive_timeout.
            emu_instance_dir: Directory where emulators are stored.
            usb_socket_path: Path to socket used to communicate with the USB
                protocol driver.
            usb_driver_autostart: Whether to start the USB protocol driver if it
                isn't running.
            ssh_private_keys: Explicit list of SSH private keys from the environment to
                provide to FFX. If left empty, setup will search the os.environ for fallback
                keys instead of relying on the strict-mode suppressed default ffx configurations.
            ssh_public_keys: Explicit list of SSH public keys. If left empty, setup will
                fallback to .pub extension of private keys if they exist.
            ssh_auth_sock: Path to SSH authentication socket for connection.
            identities_only: Whether to strictly use explicit identity files.
                If None, checks IDENTITIES_ONLY or SSH_IDENTITIES_ONLY environment variable.

        Raises:
            FfxConfigError: If setup has already been called once.
        """
        if self._setup_done:
            raise FfxConfigError("setup has already been called once.")

        # Ensure clean up occurs upon normal program termination.
        atexit.register(self._atexit_callback)

        self._ffx_binary: str = binary_path if binary_path else _FFX_BINARY
        self._isolate_dir: fuchsia_controller.IsolateDir | None = (
            fuchsia_controller.IsolateDir(isolate_dir)
        )
        self._logs_dir: str = logs_dir
        self._logs_level: str | None = logs_level
        self._subtools_search_path: str | None = subtools_search_path
        self._proxy_timeout_secs: int | None = proxy_timeout_secs
        self._ssh_keepalive_timeout: int | None = ssh_keepalive_timeout
        self._enable_usb: bool = enable_usb
        self._usb_socket_path: str | None = usb_socket_path
        self._usb_driver_autostart: bool = usb_driver_autostart
        self._emu_instance_dir: str | None = emu_instance_dir
        self._ssh_auth_sock: str | None = (
            ssh_auth_sock
            if ssh_auth_sock is not None
            else os.environ.get("SSH_AUTH_SOCK")
        )
        if identities_only is None:
            env_val = (
                os.environ.get("IDENTITIES_ONLY")
                or os.environ.get("SSH_IDENTITIES_ONLY")
                or os.environ.get("FUCHSIA_SSH_IDENTITIES_ONLY")
            )
            if env_val is not None:
                identities_only = env_val.lower() in (
                    "1",
                    "true",
                    "yes",
                    "y",
                )
        self._identities_only: bool | None = identities_only

        # Use explicitly provided keys or fallback to os.environ keys
        priv_keys: list[str] = (
            list(ssh_private_keys) if ssh_private_keys else []
        )
        env_ssh_key = os.environ.get("FUCHSIA_SSH_KEY")
        if env_ssh_key and env_ssh_key not in priv_keys:
            priv_keys.append(env_ssh_key)

        if not priv_keys:
            default_priv = os.path.expanduser("~/.ssh/fuchsia_ed25519")
            if os.path.exists(default_priv):
                priv_keys.append(default_priv)

        pub_keys: list[str] = list(ssh_public_keys) if ssh_public_keys else []
        if not pub_keys:
            for key in priv_keys:
                pub_key = f"{key}.pub"
                if os.path.exists(pub_key):
                    pub_keys.append(pub_key)
                else:
                    _LOGGER.warning(
                        "Public key '%s' not found for private key '%s'",
                        pub_key,
                        key,
                    )

        env_auth_keys = os.environ.get("FUCHSIA_AUTHORIZED_KEYS")
        if env_auth_keys and env_auth_keys not in pub_keys:
            pub_keys.append(env_auth_keys)

        if not pub_keys:
            default_pub = os.path.expanduser("~/.ssh/fuchsia_authorized_keys")
            if os.path.exists(default_pub):
                pub_keys.append(default_pub)

        # Set these to the resolved lists (which may be empty, e.g., []).
        # An empty list overrides the ffx defaults that contain variable mappings (e.g., $HOME),
        # preventing ffx strict mode from failing on variable expansions.
        self._ssh_private_keys: list[str] | None = priv_keys
        self._ssh_public_keys: list[str] | None = pub_keys

        self._setup_done = True

    def close(self) -> None:
        """Clean up method.

        Raises:
            FfxConfigError: When called before calling `FfxConfig.setup`
        """
        if self._setup_done is False:
            raise FfxConfigError("close called before calling setup.")

        # Setting to None will delete the `self._isolate_dir.directory()`
        self._isolate_dir = None

        self._setup_done = False

    def get_config(self) -> FfxConfigData:
        """Returns the FFX configuration information that has been set.

        Returns:
            FfxConfigData

        Raises:
            FfxConfigError: When called before `FfxConfig.setup` or after `FfxConfig.close`.
        """
        if self._setup_done is False:
            raise FfxConfigError("get_config called before calling setup.")
        if self._isolate_dir is None:
            raise FfxConfigError("get_config called after calling close.")

        return FfxConfigData(
            binary_path=self._ffx_binary,
            isolate_dir=self._isolate_dir,
            logs_dir=self._logs_dir,
            logs_level=self._logs_level,
            subtools_search_path=self._subtools_search_path,
            proxy_timeout_secs=self._proxy_timeout_secs,
            ssh_keepalive_timeout=self._ssh_keepalive_timeout,
            enable_usb=self._enable_usb,
            usb_socket_path=self._usb_socket_path,
            usb_driver_autostart=self._usb_driver_autostart,
            emu_instance_dir=self._emu_instance_dir,
            ssh_private_keys=self._ssh_private_keys,
            ssh_public_keys=self._ssh_public_keys,
            ssh_auth_sock=self._ssh_auth_sock,
            identities_only=self._identities_only,
        )

    def _atexit_callback(self) -> None:
        try:
            self.close()
        except FfxConfigError:
            pass
