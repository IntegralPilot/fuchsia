#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from typing import Literal

from antlion.controllers import pdu
from mobly import signals

# Create an optional dependency for dlipower since it has a transitive
# dependency on beautifulsoup4. This library is difficult to maintain as a
# third_party dependency in Fuchsia since it is hosted on launchpad.
#
# TODO(b/246999212): Explore alternatives to the dlipower package
try:
    import dlipower

    HAS_IMPORT_DLIPOWER = True
except ImportError:
    HAS_IMPORT_DLIPOWER = False


class PduDevice(pdu.PduDevice):
    """Implementation of pure abstract PduDevice object for the Digital Loggers
    WebPowerSwitch PDUs.

    This controller supports the following Digital Loggers PDUs:
        - Pro (VII)
        - WebPowerSwitch V
        - WebPowerSwitch IV
        - WebPowerSwitch III
        - WebPowerSwitch II
        - Ethernet Power Controller III
    """

    def __init__(
        self, host: str, username: str | None, password: str | None
    ) -> None:
        """
        Note: This may require allowing plaintext password sign in on the
        power switch, which can be configure in the device's control panel.
        """
        super(PduDevice, self).__init__(host, username, password)

        if not HAS_IMPORT_DLIPOWER:
            raise signals.ControllerError(
                "Digital Loggers PDUs are not supported with current installed "
                "packages; install the dlipower package to add support"
            )

        self.power_switch = dlipower.PowerSwitch(
            hostname=host, userid=username, password=password
        )
        # Connection is made at command execution, this verifies the device
        # can be reached before continuing.
        if not self.power_switch.statuslist():
            raise pdu.PduError(
                "Failed to connect get WebPowerSwitch status. Incorrect host, "
                "userid, or password?"
            )
        else:
            self.log.info(f"Connected to WebPowerSwitch ({host}).")

    def on_all(self) -> None:
        """Turn on power to all outlets."""
        for outlet in self.power_switch:
            outlet.on()
            self._verify_state(outlet.name, "ON")

    def off_all(self) -> None:
        """Turn off power to all outlets."""
        for outlet in self.power_switch:
            outlet.off()
            self._verify_state(outlet.name, "OFF")

    def on(self, outlet: str | int) -> None:
        """Turn on power to given outlet

        Args:
            outlet: string or int, the outlet name/number
        """
        self.power_switch.command_on_outlets("on", str(outlet))
        self._verify_state(outlet, "ON")

    def off(self, outlet: str | int) -> None:
        """Turn off power to given outlet

        Args:
            outlet: string or int, the outlet name/number
        """
        self.power_switch.command_on_outlets("off", str(outlet))
        self._verify_state(outlet, "OFF")

    def reboot(self, outlet: str | int) -> None:
        """Cycle the given outlet to OFF and back ON.

        Args:
            outlet: string or int, the outlet name/number
        """
        self.power_switch.command_on_outlets("cycle", str(outlet))
        self._verify_state(outlet, "ON")

    def status(self) -> dict[str, bool]:
        """Return the status of the switch outlets.

        Return:
            a dict mapping outlet string numbers to:
                True if outlet is ON
                False if outlet is OFF
        """
        status_list = self.power_switch.statuslist()
        return {str(outlet): state == "ON" for outlet, _, state in status_list}

    def close(self) -> None:
        # Since there isn't a long-running connection, close is not applicable.
        pass

    def _verify_state(
        self,
        outlet: str | int,
        expected_state: Literal["ON"] | Literal["OFF"],
        timeout: int = 3,
    ) -> None:
        """Verify that the state of a given outlet is at an expected state.
        There can be a slight delay in when the device receives the
        command and when the state actually changes (especially when powering
        on). This function is used to verify the change has occurred before
        exiting.

        Args:
            outlet: string, the outlet name or number to check state.
            expected_state: string, 'ON' or 'OFF'

        Returns if actual state reaches expected state.

        Raises:
            PduError: if state has not reached expected state at timeout.
        """
        actual_state = None
        for _ in range(timeout):
            actual_state = self.power_switch.status(str(outlet))
            if actual_state == expected_state:
                return
            else:
                self.log.debug(
                    f"Outlet {outlet} not yet in state {expected_state}"
                )
        raise pdu.PduError(
            "Outlet %s on WebPowerSwitch (%s) failed to reach expected state. \n"
            "Expected State: %s\n"
            "Actual State: %s"
            % (outlet, self.host, expected_state, actual_state)
        )
