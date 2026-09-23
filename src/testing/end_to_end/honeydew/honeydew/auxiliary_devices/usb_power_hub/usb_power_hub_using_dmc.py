# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""UsbPower auxiliary device implementation using DMC."""

import enum
import logging
import os
import platform

from honeydew import errors
from honeydew.auxiliary_devices.usb_power_hub import usb_power_hub
from honeydew.utils import host_shell

_LOGGER: logging.Logger = logging.getLogger(__name__)

DMC_PATH_KEY: str = "DMC_PATH"


class UsbPowerDmcError(usb_power_hub.UsbPowerHubError):
    """Custom exception class for raising DMC related errors."""


class UsbPowerState(enum.StrEnum):
    """Different power states supported by `dmc set-usb-power-state`."""

    ON = "on"
    OFF = "off"


class UsbPowerHubUsingDmc(usb_power_hub.UsbPowerHub):
    """UsbPowerHub auxiliary device implementation using DMC.

    Note: `UsbPowerUsingDmc` is implemented to do power off/on the usb for a
    Fuchsia device that is hosted in Fuchsia Infra labs and Fuchsia Infra
    workflows are different from local workflows. As a result,
    `UsbPowerUsingDmc` will only work for FuchsiaInfra labs and will not work
    for local set ups or any other setups for that matter.

    Args:
        device_name: Device name returned by `ffx target list`.
    """

    def __init__(self, device_name: str) -> None:
        super().__init__()
        self._name: str = device_name

        try:
            self._dmc_path: str = os.environ[DMC_PATH_KEY]
        except KeyError as error:
            raise UsbPowerDmcError(
                f"{DMC_PATH_KEY} environmental variable is not set on "
                f"{platform.node()}"
            ) from error

    # List all the public methods
    def power_off(self, port: int | None = None) -> None:
        """Turns off the usb power to the Fuchsia device.

        Args:
            port: None. Not being used by this implementation.

        Raises:
            UsbPowerError: In case of failure.
        """
        _LOGGER.info("Powering off usb for %s...", self._name)
        self._run(
            command=self._generate_dmc_usb_power_state_cmd(
                power_state=UsbPowerState.OFF
            )
        )
        _LOGGER.info("Successfully powered off usb for %s...", self._name)

    def power_on(self, port: int | None = None) -> None:
        """Turns on the usb power to the Fuchsia device.

        Args:
            port: None. Not being used by this implementation.

        Raises:
            UsbPowerError: In case of failure.
        """
        _LOGGER.info("Powering on usb for %s...", self._name)
        self._run(
            command=self._generate_dmc_usb_power_state_cmd(
                power_state=UsbPowerState.ON
            )
        )
        _LOGGER.info("Successfully powered on usb for %s...", self._name)

    def _generate_dmc_usb_power_state_cmd(
        self, power_state: UsbPowerState
    ) -> list[str]:
        """Helper method that takes the UsbPowerState and generates the DMC
        power_state command in the format accepted by `_run()` method.

        Args:
            power_state: UsbPowerState

        Returns:
            DMC power_state command
        """
        return (
            f"{self._dmc_path} set-usb-power-state "
            f"-nodename {self._name} "
            f"-state {power_state}"
        ).split()

    def _run(self, command: list[str]) -> None:
        """Helper method to run a command and returns the output.

        Args:
            command: Command to run.

        Raises:
            UsbPowerHubError: In case of failure.
        """
        try:
            host_shell.run(cmd=command)
        except errors.HostCmdError as err:
            raise usb_power_hub.UsbPowerHubError(err) from err
