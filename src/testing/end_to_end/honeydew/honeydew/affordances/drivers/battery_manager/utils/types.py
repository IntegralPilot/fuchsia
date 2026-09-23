# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Data types used by Battery affordance."""

from __future__ import annotations

import enum
from dataclasses import dataclass
from datetime import timedelta

import fidl_fuchsia_hardware_power_battery as f_battery
import fidl_fuchsia_hardware_power_source as f_power_source


class PowerSourceType(enum.IntEnum):
    """Corresponds to fuchsia.hardware.power.source.SourceType."""

    AC = 1
    BATTERY = 2
    USB = 3


class ChargeStatus(enum.IntEnum):
    """Corresponds to fuchsia.hardware.power.battery.ChargeStatus."""

    NOT_CHARGING = 1
    CHARGING = 2
    DISCHARGING = 3
    FULL = 4


class HealthStatus(enum.IntEnum):
    """Corresponds to fuchsia.hardware.power.battery.HealthStatus."""

    GOOD = 1
    COLD = 2
    COOL = 3
    WARM = 4
    HOT = 5
    DEAD = 6
    OVER_VOLTAGE = 7
    UNSPECIFIED_FAILURE = 8


@dataclass(frozen=True)
class PowerSourceSpec:
    """Base spec from underlying power source."""

    name: str | None = None
    manufacturer: str | None = None
    model_name: str | None = None
    type: PowerSourceType | None = None

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Spec) -> PowerSourceSpec:
        source_type = (
            PowerSourceType(fidl.type_) if fidl.type_ is not None else None
        )
        return cls(
            name=fidl.name,
            manufacturer=fidl.manufacturer,
            model_name=fidl.model_name,
            type=source_type,
        )

    def to_fidl(self) -> f_power_source.Spec:
        source_type = (
            f_power_source.SourceType(self.type.value)
            if self.type is not None
            else None
        )
        return f_power_source.Spec(
            name=self.name,
            manufacturer=self.manufacturer,
            model_name=self.model_name,
            type_=source_type,
        )


@dataclass(frozen=True)
class PowerSourceStatus:
    """Base dynamic status from underlying power source."""

    present: bool | None = None
    voltage_uv: int | None = None
    current_ua: int | None = None

    @classmethod
    def from_fidl(cls, fidl: f_power_source.Status) -> PowerSourceStatus:
        return cls(
            present=fidl.present,
            voltage_uv=fidl.voltage_uv,
            current_ua=fidl.current_ua,
        )

    def to_fidl(self) -> f_power_source.Status:
        return f_power_source.Status(
            present=self.present,
            voltage_uv=self.voltage_uv,
            current_ua=self.current_ua,
        )


@dataclass(frozen=True)
class BatteryStatus:
    """Dynamic status of the battery pack telemetry."""

    present: bool | None = None
    voltage_uv: int | None = None
    current_ua: int | None = None
    charge_status: ChargeStatus | None = None
    level_percent: float | None = None
    remaining_capacity_uah: int | None = None
    full_charge_capacity_uah: int | None = None
    health: HealthStatus | None = None
    temp_celsius: float | None = None
    cycle_count: int | None = None
    time_remaining: timedelta | None = None

    @property
    def temperature_mc(self) -> int | None:
        """Battery temperature in millidegrees Celsius for backwards compatibility."""
        if self.temp_celsius is None:
            return None
        return round(self.temp_celsius * 1000)

    @classmethod
    def from_fidl(cls, fidl: f_battery.Status) -> BatteryStatus:
        charge_status = (
            ChargeStatus(fidl.charge_status)
            if fidl.charge_status is not None
            else None
        )
        health = HealthStatus(fidl.health) if fidl.health is not None else None
        time_rem = (
            timedelta(microseconds=fidl.time_remaining // 1000)
            if fidl.time_remaining is not None
            else None
        )
        return cls(
            present=fidl.present,
            voltage_uv=fidl.voltage_uv,
            current_ua=fidl.current_ua,
            charge_status=charge_status,
            level_percent=fidl.level_percent,
            remaining_capacity_uah=fidl.remaining_capacity_uah,
            full_charge_capacity_uah=fidl.full_charge_capacity_uah,
            health=health,
            temp_celsius=fidl.temp_celsius,
            cycle_count=fidl.cycle_count,
            time_remaining=time_rem,
        )

    def to_fidl(self) -> f_battery.Status:
        charge_status = (
            f_battery.ChargeStatus(self.charge_status.value)
            if self.charge_status is not None
            else None
        )
        health = (
            f_battery.HealthStatus(self.health.value)
            if self.health is not None
            else None
        )
        time_rem_ns = (
            int(self.time_remaining.total_seconds() * 1_000_000_000)
            if self.time_remaining is not None
            else None
        )
        return f_battery.Status(
            present=self.present,
            voltage_uv=self.voltage_uv,
            current_ua=self.current_ua,
            charge_status=charge_status,
            level_percent=self.level_percent,
            remaining_capacity_uah=self.remaining_capacity_uah,
            full_charge_capacity_uah=self.full_charge_capacity_uah,
            health=health,
            temp_celsius=self.temp_celsius,
            cycle_count=self.cycle_count,
            time_remaining=time_rem_ns,
        )


@dataclass(frozen=True)
class WatchOptions:
    """Configuration options for battery state watching."""

    interest: BatteryStatus | None = None
    wake_on: BatteryStatus | None = None

    @classmethod
    def from_fidl(cls, fidl: f_battery.WatchOptions) -> WatchOptions:
        interest = (
            BatteryStatus.from_fidl(fidl.interest)
            if fidl.interest is not None
            else None
        )
        wake_on = (
            BatteryStatus.from_fidl(fidl.wake_on)
            if fidl.wake_on is not None
            else None
        )
        return cls(
            interest=interest,
            wake_on=wake_on,
        )

    def to_fidl(self) -> f_battery.WatchOptions:
        interest = self.interest.to_fidl() if self.interest else None
        wake_on = self.wake_on.to_fidl() if self.wake_on else None
        return f_battery.WatchOptions(
            interest=interest,
            wake_on=wake_on,
        )


@dataclass(frozen=True)
class BatterySpec:
    """Static hardware characteristics of the battery pack."""

    design_capacity_uah: int | None = None
    design_voltage_uv: int | None = None
    chemistry: str | None = None
    model: str | None = None
    supported_options: WatchOptions | None = None

    @classmethod
    def from_fidl(cls, fidl: f_battery.Spec) -> BatterySpec:
        supported_options = (
            WatchOptions.from_fidl(fidl.supported_options)
            if fidl.supported_options is not None
            else None
        )
        return cls(
            design_capacity_uah=fidl.design_capacity_uah,
            design_voltage_uv=fidl.design_voltage_uv,
            chemistry=fidl.chemistry,
            model=fidl.model,
            supported_options=supported_options,
        )

    def to_fidl(self) -> f_battery.Spec:
        supported_options = (
            self.supported_options.to_fidl() if self.supported_options else None
        )
        return f_battery.Spec(
            design_capacity_uah=self.design_capacity_uah,
            design_voltage_uv=self.design_voltage_uv,
            chemistry=self.chemistry,
            model=self.model,
            supported_options=supported_options,
        )
