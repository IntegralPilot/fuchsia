# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""PowerSwitch auxiliary device implementation using a Power Distribution Unit (PDU) via SSH."""

import enum
import logging
import os
import shlex

from honeydew import errors
from honeydew.auxiliary_devices.power_switch import power_switch
from honeydew.utils import host_shell

_LOGGER: logging.Logger = logging.getLogger(__name__)


class PowerSwitchPduError(power_switch.PowerSwitchError):
    """Custom exception class for raising PDU related errors."""


class PowerStatePdu(enum.StrEnum):
    """Different power states supported by the PDU uom command."""

    ON = "true"
    OFF = "false"


class PowerSwitchUsingPdu(power_switch.PowerSwitch):
    """PowerSwitch auxiliary device implementation using a PDU via SSH.

    Note: `PowerSwitchPdu` is implemented to do power off/on on a Fuchsia device
    that is hosted locally. This will not work for infra setups.

    Usage Example :
    uom set "relay/outlets/7/state" "true"  -> Turn on port 8.
    uom set "relay/outlets/7/state" "false" -> Turn off port 8.

    since port numbers starts from 0.

    Args:
        pdu_host: The IP address or hostname of the PDU.
        pdu_username: The username for SSH connection to the PDU.
        priv_key_path: The absolute path to the SSH private key file.
    """

    def __init__(
        self,
        pdu_host: str,
        pdu_username: str,
        priv_key_path: str,
    ) -> None:
        self._host: str = pdu_host
        self._username: str = pdu_username
        self._priv_key_path: str = priv_key_path

        if not os.path.exists(self._priv_key_path):
            raise PowerSwitchPduError(
                f"SSH private key not found at path: {self._priv_key_path}"
            )

        _LOGGER.info(
            "Initialized PowerSwitchUsingPdu for host %s with user %s and key %s",
            self._host,
            self._username,
            self._priv_key_path,
        )

    def power_off(self, outlet: int | None = None) -> None:
        """Turns off the power at the specified outlet on the PDU.

        Args:
            outlet: The outlet number on the PDU. Must not be None.

        Raises:
            PowerSwitchError: If outlet is None or the SSH command fails.
        """
        if outlet is None:
            raise PowerSwitchPduError(
                "Outlet number must be specified for PDU operation."
            )

        _LOGGER.info(
            "PDU is powering off outlet %d on %s...", outlet, self._host
        )

        command = self._generate_pdu_ssh_cmd(
            outlet=outlet, power_state=PowerStatePdu.OFF
        )
        self.run(command=command)
        _LOGGER.info(
            "Successfully powered off outlet %d on %s.", outlet, self._host
        )

    def power_on(self, outlet: int | None = None) -> None:
        """Turns on the power at the specified outlet on the PDU.

        Args:
            outlet: The outlet number on the PDU. Must not be None.

        Raises:
            PowerSwitchError: If outlet is None or the SSH command fails.
        """
        if outlet is None:
            raise PowerSwitchPduError(
                "Outlet number must be specified for PDU operation."
            )

        _LOGGER.info(
            "PDU is powering on outlet %d on %s...", outlet, self._host
        )

        command = self._generate_pdu_ssh_cmd(
            outlet=outlet, power_state=PowerStatePdu.ON
        )
        self.run(command=command)
        _LOGGER.info(
            "Successfully powered on outlet %d on %s.", outlet, self._host
        )

    def _generate_pdu_ssh_cmd(self, outlet: int, power_state: str) -> list[str]:
        """Helper method that generates the SSH command to control the PDU outlet.

        Args:
            outlet: The outlet number to control.
            power_state: 'true' (ON) or 'false' (OFF) for the PDU command.

        Returns:
            SSH command split into a list format for host_shell.run().
        """
        remote_command = f"uom set relay/outlets/{outlet}/state {power_state}"

        ssh_command = (
            f"ssh -i {self._priv_key_path} {self._username}@{self._host} "
            f"-o StrictHostKeyChecking=no "
            f"-o UserKnownHostsFile=/dev/null "
            f"'{remote_command}'"
        )

        return shlex.split(ssh_command)

    def run(self, command: list[str]) -> None:
        """Helper method to run a command and returns the output using host_shell.run.

        Args:
            command: Command to run (list of strings).

        Raises:
            PowerSwitchPduError: In case of failure.
        """
        try:
            host_shell.run(cmd=command)
        except errors.HostCmdError as err:
            raise PowerSwitchPduError(err) from err
