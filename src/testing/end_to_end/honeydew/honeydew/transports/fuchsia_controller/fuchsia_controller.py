# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Provides Host-(Fuchsia)Target interactions via Fuchsia-Controller."""

import logging

import fuchsia_controller_py as fuchsia_controller
from honeydew.affordances_capable import FuchsiaDeviceIpChange
from honeydew.transports.ffx import config as ffx_config
from honeydew.transports.fuchsia_controller import errors as fc_errors
from honeydew.typing import custom_types
from honeydew.utils import decorators

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FuchsiaController:
    """Provides Host-(Fuchsia)Target interactions via Fuchsia-Controller.

    Args:
        target_name: Fuchsia device name.
        ffx_config_data: Configuration associated with FuchsiaController, FFX.
        target_ip_port: Fuchsia device IP Address.
        device_ip_change: Object that implements FuchsiaDeviceIpChange to handle Fuchsia device
            IP changes.

    Raises:
        FuchsiaControllerConnectionError: If target is not ready.
        FuchsiaControllerError: Failed to instantiate.
    """

    def __init__(
        self,
        target_name: str,
        ffx_config_data: ffx_config.FfxConfigData,
        target_ip_port: custom_types.IpPort | None = None,
        device_ip_change: FuchsiaDeviceIpChange | None = None,
        target_serial: str | None = None,
    ) -> None:
        self._target_name: str = target_name

        self._ffx_config_data: ffx_config.FfxConfigData = ffx_config_data

        self._target_ip_port: custom_types.IpPort | None = target_ip_port
        if target_ip_port is not None and device_ip_change is None:
            raise ValueError(
                "Pass 'device_ip_change' argument also when 'target_ip_port' arg is passed"
            )
        self._device_ip_change: FuchsiaDeviceIpChange | None = device_ip_change
        if self._device_ip_change:
            self._device_ip_change.register_for_on_device_ip_change(
                fn=self._on_device_ip_change
            )

        self._target_serial: str | None = target_serial

        self._target: str
        if self._target_ip_port:
            self._target = str(self._target_ip_port)
        elif self._target_serial:
            # FFX/Fuchsia-Controller uses "id:<serial-number>" addressing scheme.
            self._target = f"id:{self._target_serial}"
        else:
            self._target = self._target_name

        self.ctx: fuchsia_controller.Context

        self.create_context()
        self.check_connection()

    def create_context(self) -> None:
        """Creates the fuchsia-controller context.

        Raises:
            FuchsiaControllerError: Failed to create FuchsiaController Context.
            FuchsiaControllerConnectionError: If target is not ready.
        """
        try:
            # To run Fuchsia-Controller in isolation
            isolate_dir: fuchsia_controller.IsolateDir | None = (
                self._ffx_config_data.isolate_dir
            )
            config: dict[str, str] = {}
            if self._ffx_config_data.logs_level:
                level = self._ffx_config_data.logs_level
                _LOGGER.debug("log.level set to %s", level)
                config["log.level"] = level
            else:
                _LOGGER.debug("log level not set.")
            if self._ffx_config_data.logs_dir:
                log_dir = self._ffx_config_data.logs_dir
                _LOGGER.debug("log.dir set to %s", log_dir)
                config["log.dir"] = log_dir
            else:
                _LOGGER.debug("log dir not set.")
            if self._ffx_config_data.enable_usb:
                enable_usb = "true"
            else:
                enable_usb = "false"
            _LOGGER.debug("connectivity.enable_usb set to %s", enable_usb)
            config["connectivity.enable_usb"] = enable_usb
            if self._ffx_config_data.usb_driver_autostart:
                usb_driver_autostart = "true"
            else:
                usb_driver_autostart = "false"
            _LOGGER.debug(
                "connectivity.usb_driver_autostart set to %s",
                usb_driver_autostart,
            )
            config["connectivity.usb_driver_autostart"] = usb_driver_autostart
            if self._ffx_config_data.usb_socket_path:
                usb_socket_path = self._ffx_config_data.usb_socket_path
                _LOGGER.debug(
                    "connectivity.usb_socket_path set to %s", usb_socket_path
                )
                config["connectivity.usb_socket_path"] = usb_socket_path
            else:
                _LOGGER.debug("connectivity.usb_socket_path not set.")
            if self._ffx_config_data.ssh_auth_sock:
                ssh_auth_sock = self._ffx_config_data.ssh_auth_sock
                _LOGGER.debug("ssh.auth-sock set to %s", ssh_auth_sock)
                config["ssh.auth-sock"] = ssh_auth_sock
            else:
                _LOGGER.debug("ssh auth sock not set.")
            if self._ffx_config_data.identities_only is not None:
                identities_only = (
                    "true" if self._ffx_config_data.identities_only else "false"
                )
                _LOGGER.debug("ssh.identities-only set to %s", identities_only)
                config["ssh.identities-only"] = identities_only
            else:
                _LOGGER.debug("ssh identities only not set.")
            msg: str = (
                f"Creating Fuchsia-Controller Context with "
                f"target='{self._target}', config='{config}'"
            )
            if isolate_dir:
                msg = f"{msg}, isolate_dir={isolate_dir.directory()}"
            _LOGGER.debug(msg)
            self.ctx = fuchsia_controller.Context(
                config=config, isolate_dir=isolate_dir, target=self._target
            )
        except Exception as err:  # pylint: disable=broad-except
            raise fc_errors.FuchsiaControllerError(
                "Failed to create Fuchsia-Controller context"
            ) from err

    @decorators.liveness_check
    def check_connection(self) -> None:
        """Checks the Fuchsia-Controller connection from host to Fuchsia device.

        Raises:
            FuchsiaControllerConnectionError
        """
        try:
            _LOGGER.debug(
                "Waiting for for Fuchsia-Controller to check the "
                "connection from host to %s...",
                self._target_name,
            )
            self.ctx.target_wait(timeout=0)
            _LOGGER.debug(
                "Fuchsia-Controller completed the connection check from host "
                "to %s...",
                self._target_name,
            )
        except Exception as err:  # pylint: disable=broad-except
            raise fc_errors.FuchsiaControllerConnectionError(
                f"Fuchsia-Controller connection check failed for "
                f"{self._target_name} with error: {err}"
            )

    def connect_device_proxy(
        self, fidl_end_point: custom_types.FidlEndpoint
    ) -> fuchsia_controller.Channel:
        """Opens a proxy to the specified FIDL end point.

        Args:
            fidl_end_point: FIDL end point tuple containing moniker and protocol
              name.

        Raises:
            FuchsiaControllerError: On FIDL communication failure.

        Returns:
            FIDL channel to proxy.
        """
        try:
            return self.ctx.connect_device_proxy(
                fidl_end_point.moniker, fidl_end_point.protocol
            )
        except fuchsia_controller.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller FIDL Error"
            ) from status

    def _on_device_ip_change(self, target_ip_port: custom_types.IpPort) -> None:
        """Callback method that gets invoked when device ip address changes.

        Args:
            target_ip_port: New IP address of the device.
        """
        self._target_ip_port = target_ip_port
        self._target = str(self._target_ip_port)

        self.create_context()
        self.check_connection()

    def before_usb_disconnect(self) -> None:
        """Callback method that gets invoked before USB disconnect."""

    def after_usb_reconnect(self) -> None:
        """Callback method that gets invoked after USB reconnect."""
        self.create_context()
        self.check_connection()

    def channel_create(
        self,
    ) -> tuple[fuchsia_controller.Channel, fuchsia_controller.Channel]:
        """Opens a pair of connected channels, usef for FIDL endpoints.

        Raises: FuchsiaControllerError: On failure to create the channels.
        """
        try:
            return self.ctx.channel_create()
        except fuchsia_controller.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller Channel Create Error"
            ) from status

    def socket_create(
        self,
    ) -> tuple[fuchsia_controller.Socket, fuchsia_controller.Socket]:
        """Opens a pair of connected sockets, used for FIDL endpoints.

        Raises: FuchsiaControllerError: On failure to create the sockets.
        """
        try:
            return self.ctx.socket_create()
        except fuchsia_controller.FcTransportStatus as status:
            raise fc_errors.FuchsiaControllerError(
                "Fuchsia Controller Socket Create Error"
            ) from status
