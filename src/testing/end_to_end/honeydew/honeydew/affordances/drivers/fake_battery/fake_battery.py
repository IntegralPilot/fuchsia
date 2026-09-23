# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for FakeBattery affordance."""

from __future__ import annotations

import abc

from honeydew.affordances import affordance
from honeydew.affordances.drivers.battery_manager.utils import types


class FakeBattery(affordance.Affordance):
    """Abstract base class for FakeBattery affordance to inspect and control the
    simulated battery and power source states on the fake-battery driver."""

    @abc.abstractmethod
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

    @abc.abstractmethod
    def get(self) -> str:
        """Queries and displays current fake battery telemetry from the driver.

        Returns:
            Command output string from fake-battery-cli get.

        Raises:
            FakeBatteryCommandError: If the fake-battery-cli command fails.
        """

    @abc.abstractmethod
    def set_level(self, level: float) -> str:
        """Sets the battery charge level percentage.

        Args:
            level: Battery charge level percent (0.0 to 100.0).

        Returns:
            Command output string.
        """

    @abc.abstractmethod
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

    @abc.abstractmethod
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

    @abc.abstractmethod
    def set_health(self, health: types.HealthStatus | str) -> str:
        """Sets the battery health state.

        Args:
            health: Health status (e.g. HealthStatus.HOT, "over_voltage").

        Returns:
            Command output string.
        """

    @abc.abstractmethod
    def set_temperature(self, temp_mc: int) -> str:
        """Sets the battery temperature.

        Args:
            temp_mc: Temperature in milli-Celsius (e.g. 45000 for 45.0°C).

        Returns:
            Command output string.
        """
