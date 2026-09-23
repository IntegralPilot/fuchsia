# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""FakeBattery affordance implementation using FFX."""

from __future__ import annotations

import logging

from honeydew import errors
from honeydew.affordances.drivers.battery_manager.utils import types
from honeydew.affordances.drivers.fake_battery import (
    errors as fake_battery_errors,
)
from honeydew.affordances.drivers.fake_battery import (
    fake_battery,
)
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types

_LOGGER: logging.Logger = logging.getLogger(__name__)

_TOOL_URL: str = "fuchsia-pkg://fuchsia.com/fake_battery_cli_pkg"
_REQUIRED_CAPABILITY: str = "test.hardwarepowercontrol.Service"
_DEFAULT_MONIKER: str = "fake-battery"


class FakeBatteryUsingFfx(fake_battery.FakeBattery):
    """FakeBattery affordance implementation using FFX component explore to invoke
    the fake-battery-cli tool.

    Args:
        device_name: Device name.
        ffx: FFX transport instance.
    """

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
    ) -> None:
        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx
        self._moniker: str = _DEFAULT_MONIKER

        self.verify_supported()

    def verify_supported(self) -> None:
        """Verifies that the FakeBattery affordance is supported on the target device.

        Raises:
            NotSupportedError: If fake-battery driver is not found on the device.
        """
        try:
            output = self._ffx.run(
                ["component", "capability", _REQUIRED_CAPABILITY],
                machine=ffx_types.MachineFormat.RAW,
            )
            if _REQUIRED_CAPABILITY not in output:
                raise errors.NotSupportedError(
                    f"Capability '{_REQUIRED_CAPABILITY}' not supported on {self._device_name}"
                )
            self._moniker = self._parse_moniker_from_capability_output(output)
        except ffx_errors.FfxCommandError as err:
            raise errors.NotSupportedError(
                f"FakeBattery affordance not supported on {self._device_name}"
            ) from err

    def _parse_moniker_from_capability_output(self, output: str) -> str:
        """Parses the active fake-battery driver moniker from capability output."""
        for line in output.splitlines():
            line = line.strip()
            if (
                f"declared capability `{_REQUIRED_CAPABILITY}`" in line
                and "devfs_driver" not in line
            ):
                parts = line.split("`")
                if len(parts) >= 2 and parts[1] != _REQUIRED_CAPABILITY:
                    return parts[1]
            if (
                f"exposed capability `{_REQUIRED_CAPABILITY}`" in line
                or f"exposed `{_REQUIRED_CAPABILITY}`" in line
            ):
                parts = line.split("`")
                if len(parts) >= 2 and parts[1] != _REQUIRED_CAPABILITY:
                    return parts[1]
        return _DEFAULT_MONIKER

    def _run_tool_command(self, cmd_args: list[str]) -> str:
        """Executes fake-battery-cli inside the driver component namespace.

        Args:
            cmd_args: List of command line arguments for the tool.

        Returns:
            Output from the tool.

        Raises:
            FakeBatteryCommandError: If the command fails to execute.
        """
        command_str = " ".join(cmd_args)
        _LOGGER.debug(
            "Executing fake-battery-cli command on %s (%s): %s",
            self._device_name,
            self._moniker,
            command_str,
        )
        try:
            return self._ffx.run(
                [
                    "component",
                    "explore",
                    self._moniker,
                    "--tools",
                    _TOOL_URL,
                    "-c",
                    command_str,
                ],
                machine=ffx_types.MachineFormat.RAW,
            )
        except ffx_errors.FfxCommandError as err:
            raise fake_battery_errors.FakeBatteryCommandError(
                f"Command '{command_str}' failed on {self._device_name}: {err}"
            ) from err

    def set(
        self,
        level: float | None = None,
        status: types.ChargeStatus | str | None = None,
        source: types.PowerSourceType | str | None = None,
        voltage_mv: int | None = None,
        current_ua: int | None = None,
        temp_mc: int | None = None,
        health: types.HealthStatus | str | None = None,
        remaining_uah: int | None = None,
        full_capacity_uah: int | None = None,
        time_remaining_sec: int | None = None,
        cycle_count: int | None = None,
    ) -> str:
        """Injects simulated battery telemetry and power source state into the
        fake-battery driver.

        Args:
            level: Battery charge level percent (0.0 to 100.0).
            status: Battery charging status ("charging", "discharging",
                "not_charging", "full" or ChargeStatus enum).
            source: Power source type ("ac", "usb", "battery", "none",
                "disconnected" or PowerSourceType enum).
            voltage_mv: Battery/source voltage in millivolts (e.g. 4200 for 4.2V).
            current_ua: Current in microamps (positive = charging, negative =
                discharging).
            temp_mc: Temperature in milli-Celsius (e.g. 25000 for 25.0°C).
            health: Battery health ("good", "cold", "cool", "warm", "hot",
                "dead", "over_voltage", "unspecified_failure" or HealthStatus
                enum).
            remaining_uah: Remaining capacity in microamp-hours.
            full_capacity_uah: Full charge capacity in microamp-hours.
            time_remaining_sec: Estimated time remaining in seconds until empty/full.
            cycle_count: Battery charge cycle count.

        Returns:
            Command output string from fake-battery-cli.

        Raises:
            FakeBatteryCommandError: If the fake-battery-cli command fails.
            ValueError: If level is invalid or no parameters are provided.
        """
        if (
            level is None
            and status is None
            and source is None
            and voltage_mv is None
            and current_ua is None
            and temp_mc is None
            and health is None
            and remaining_uah is None
            and full_capacity_uah is None
            and time_remaining_sec is None
            and cycle_count is None
        ):
            raise ValueError(
                "At least one parameter must be provided to set() fake battery state."
            )

        if level is not None and not (0.0 <= level <= 100.0):
            raise ValueError(
                f"Level percent must be between 0.0 and 100.0, got {level}"
            )

        cmd_args: list[str] = ["fake-battery-cli", "set"]

        if level is not None:
            cmd_args.extend(["--level", str(level)])
        if status is not None:
            status_str = (
                status.name.lower()
                if isinstance(status, types.ChargeStatus)
                else str(status).lower()
            )
            cmd_args.extend(["--status", status_str])
        if source is not None:
            source_str = (
                source.name.lower()
                if isinstance(source, types.PowerSourceType)
                else str(source).lower()
            )
            cmd_args.extend(["--source", source_str])
        if voltage_mv is not None:
            cmd_args.extend(["--voltage-mv", str(voltage_mv)])
        if current_ua is not None:
            cmd_args.extend(["--current-ua", str(current_ua)])
        if temp_mc is not None:
            cmd_args.extend(["--temp-mc", str(temp_mc)])
        if health is not None:
            health_str = (
                health.name.lower()
                if isinstance(health, types.HealthStatus)
                else str(health).lower()
            )
            cmd_args.extend(["--health", health_str])
        if remaining_uah is not None:
            cmd_args.extend(["--remaining-uah", str(remaining_uah)])
        if full_capacity_uah is not None:
            cmd_args.extend(["--full-capacity-uah", str(full_capacity_uah)])
        if time_remaining_sec is not None:
            cmd_args.extend(["--time-remaining-sec", str(time_remaining_sec)])
        if cycle_count is not None:
            cmd_args.extend(["--cycle-count", str(cycle_count)])

        return self._run_tool_command(cmd_args)

    def get(self) -> str:
        """Queries and displays current fake battery telemetry from the driver.

        Returns:
            Command output string from fake-battery-cli get.

        Raises:
            FakeBatteryCommandError: If the fake-battery-cli command fails.
        """
        return self._run_tool_command(["fake-battery-cli", "get"])

    def set_level(self, level: float) -> str:
        """Sets the battery charge level percentage.

        Args:
            level: Battery charge level percent (0.0 to 100.0).

        Returns:
            Command output string.
        """
        return self.set(level=level)

    def set_charging(
        self,
        source: types.PowerSourceType | str = types.PowerSourceType.AC,
        voltage_mv: int | None = None,
        current_ua: int | None = None,
    ) -> str:
        """Simulates battery charging from a power source.

        Args:
            source: Power source type (defaults to AC).
            voltage_mv: Voltage in millivolts.
            current_ua: Charging current in microamps (positive value).

        Returns:
            Command output string.
        """
        curr = abs(current_ua) if current_ua is not None else None
        return self.set(
            status=types.ChargeStatus.CHARGING,
            source=source,
            voltage_mv=voltage_mv,
            current_ua=curr,
        )

    def set_discharging(
        self,
        current_ua: int | None = None,
        voltage_mv: int | None = None,
    ) -> str:
        """Simulates battery discharging under load.

        Args:
            current_ua: Discharging current in microamps (magnitude or negative).
            voltage_mv: Voltage in millivolts.

        Returns:
            Command output string.
        """
        curr = -abs(current_ua) if current_ua is not None else None
        return self.set(
            status=types.ChargeStatus.DISCHARGING,
            source="none",
            voltage_mv=voltage_mv,
            current_ua=curr,
        )

    def set_health(self, health: types.HealthStatus | str) -> str:
        """Sets the battery health state.

        Args:
            health: Health status (e.g. HealthStatus.HOT, "over_voltage").

        Returns:
            Command output string.
        """
        return self.set(health=health)

    def set_temperature(self, temp_mc: int) -> str:
        """Sets the battery temperature.

        Args:
            temp_mc: Temperature in milli-Celsius (e.g. 45000 for 45.0°C).

        Returns:
            Command output string.
        """
        return self.set(temp_mc=temp_mc)
