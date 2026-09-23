# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""PowerSwitch auxiliary device implementation using DMC."""

import enum
import logging
import os
import platform

from honeydew import errors
from honeydew.auxiliary_devices.power_switch import power_switch
from honeydew.utils import host_shell

_LOGGER: logging.Logger = logging.getLogger(__name__)

DMC_PATH_KEY: str = "DMC_PATH"


class PowerSwitchDmcError(power_switch.PowerSwitchError):
    """Custom exception class for raising DMC related errors."""


class PowerState(enum.StrEnum):
    """Different power states supported by `dmc set-power-state`."""

    ON = "on"
    OFF = "off"
    CYCLE = "cycle"


class PowerSwitchUsingDmc(power_switch.PowerSwitch):
    """PowerSwitch auxiliary device implementation using DMC.

    Note: `PowerSwitchDmc` is implemented to do power off/on a Fuchsia device
    that is hosted in Fuchsia Infra labs and Fuchsia Infra workflows are
    different from local workflows. As a result, `PowerSwitchDmc` will only work
    for FuchsiaInfra labs and will not work for local set ups or any other
    setups for that matter.

    Args:
        device_name: Device name returned by `ffx target list`.
    """

    def __init__(self, device_name: str) -> None:
        self._name: str = device_name

        try:
            self._dmc_path: str = os.environ[DMC_PATH_KEY]
        except KeyError as error:
            raise PowerSwitchDmcError(
                f"{DMC_PATH_KEY} environmental variable is not set on "
                f"{platform.node()}"
            ) from error

    # List all the public methods
    def power_off(self, outlet: int | None = None) -> None:
        """Turns off the power of the Fuchsia device.

        Args:
            outlet: None. Not being used by this implementation.

        Raises:
            PowerSwitchError: In case of failure.
        """
        _LOGGER.info("Lacewing is powering off %s...", self._name)
        self._run(
            command=self._generate_dmc_power_state_cmd(
                power_state=PowerState.OFF
            )
        )
        _LOGGER.info("Successfully powered off %s...", self._name)

    def power_on(self, outlet: int | None = None) -> None:
        """Turns on the power of the Fuchsia device.

        Args:
            outlet: None. Not being used by this implementation.

        Raises:
            PowerSwitchError: In case of failure.
        """
        _LOGGER.info("Powering on %s...", self._name)
        self._run(
            command=self._generate_dmc_power_state_cmd(
                power_state=PowerState.ON
            )
        )
        _LOGGER.info("Successfully powered on %s...", self._name)

    def _generate_dmc_power_state_cmd(
        self, power_state: PowerState
    ) -> list[str]:
        """Helper method that takes the PowerState and generates the DMC
        power_state command in the format accepted by `_run()` method.

        Args:
            power_state: PowerState

        Returns:
            DMC power_state command
        """
        return (
            f"{self._dmc_path} set-power-state "
            f"-nodename {self._name} "
            f"-state {power_state}"
        ).split()

    def _run(self, command: list[str]) -> None:
        """Helper method to run a command and returns the output.

        Args:
            command: Command to run.

        Raises:
            PowerSwitchError: In case of failure.
        """
        try:
            host_shell.run(cmd=command)
        except errors.HostCmdError as err:
            raise power_switch.PowerSwitchError(err) from err
