# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for FakeBattery affordance."""

from __future__ import annotations

import unittest
from unittest import mock

from honeydew import errors
from honeydew.affordances.drivers.battery_manager.utils.types import (
    ChargeStatus,
    HealthStatus,
    PowerSourceType,
)
from honeydew.affordances.drivers.fake_battery import (
    FakeBatteryUsingFfx,
)
from honeydew.affordances.drivers.fake_battery import (
    errors as fake_battery_errors,
)
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types

_TOOL_URL = "fuchsia-pkg://fuchsia.com/fake_battery_cli_pkg"
_CAPABILITY_OUTPUT = (
    "`bootstrap/boot-drivers:dev.sys.platform.fake-battery` declared capability `test.hardwarepowercontrol.Service`\n"
    "`bootstrap/boot-drivers:dev.sys.platform.fake-battery` exposed capability `test.hardwarepowercontrol.Service` from self to parent"
)


class FakeBatteryTests(unittest.TestCase):
    """Unit tests for FakeBatteryUsingFfx affordance."""

    def setUp(self) -> None:
        super().setUp()
        self.ffx_obj = mock.MagicMock(spec=ffx_transport.FFX, autospec=True)
        self.ffx_obj.run.return_value = _CAPABILITY_OUTPUT

        self.fake_battery = FakeBatteryUsingFfx(
            device_name="fuchsia-test-device",
            ffx=self.ffx_obj,
        )

    def test_verify_supported_failure_missing_capability(self) -> None:
        """Test verify_supported raises NotSupportedError when capability missing."""
        self.ffx_obj.run.return_value = ""
        with self.assertRaises(errors.NotSupportedError):
            FakeBatteryUsingFfx(
                device_name="fuchsia-test-device",
                ffx=self.ffx_obj,
            )

    def test_verify_supported_failure_ffx_error(self) -> None:
        """Test verify_supported raises NotSupportedError when FFX command fails."""
        self.ffx_obj.run.side_effect = ffx_errors.FfxCommandError(
            "Target not found"
        )
        with self.assertRaises(errors.NotSupportedError):
            FakeBatteryUsingFfx(
                device_name="fuchsia-test-device",
                ffx=self.ffx_obj,
            )

    def test_fallback_moniker_when_unparsed(self) -> None:
        """Test default moniker is used when output format doesn't contain backticks."""
        self.ffx_obj.run.return_value = (
            "Found capability test.hardwarepowercontrol.Service"
        )
        fb = FakeBatteryUsingFfx(
            device_name="fuchsia-test-device",
            ffx=self.ffx_obj,
        )
        self.assertEqual(fb._moniker, "fake-battery")

    def test_get_success(self) -> None:
        """Test get runs fake-battery-cli get and returns status text."""
        expected_output = "=== Fake Battery Status ===\n  Level: 98.7%"
        self.ffx_obj.run.return_value = expected_output

        output = self.fake_battery.get()
        self.assertEqual(output, expected_output)

        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                "fake-battery-cli get",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_get_failure(self) -> None:
        """Test get raises FakeBatteryCommandError when command fails."""
        self.ffx_obj.run.side_effect = ffx_errors.FfxCommandError(
            "explore error"
        )
        with self.assertRaises(fake_battery_errors.FakeBatteryCommandError):
            self.fake_battery.get()

    def test_set_all_options(self) -> None:
        """Test set with all CLI options provided."""
        self.ffx_obj.run.return_value = (
            "Successfully updated fake battery state."
        )

        output = self.fake_battery.set(
            level=75.5,
            status="charging",
            source="ac",
            voltage_mv=4200,
            current_ua=250000,
            temp_mc=28000,
            health="good",
            remaining_uah=300000,
            full_capacity_uah=400000,
            time_remaining_sec=3600,
            cycle_count=15,
        )
        self.assertIn("Successfully updated", output)

        expected_cmd = (
            "fake-battery-cli set --level 75.5 --status charging --source ac "
            "--voltage-mv 4200 --current-ua 250000 --temp-mc 28000 --health good "
            "--remaining-uah 300000 --full-capacity-uah 400000 "
            "--time-remaining-sec 3600 --cycle-count 15"
        )
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                expected_cmd,
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_set_with_enums(self) -> None:
        """Test set with enum instances."""
        self.ffx_obj.run.return_value = (
            "Successfully updated fake battery state."
        )

        self.fake_battery.set(
            status=ChargeStatus.CHARGING,
            source=PowerSourceType.USB,
            health=HealthStatus.HOT,
        )

        expected_cmd = (
            "fake-battery-cli set --status charging --source usb --health hot"
        )
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                expected_cmd,
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_set_no_params_raises_value_error(self) -> None:
        """Test set with no arguments raises ValueError."""
        with self.assertRaises(ValueError):
            self.fake_battery.set()

    def test_set_invalid_level_raises_value_error(self) -> None:
        """Test set with invalid level percentage raises ValueError."""
        with self.assertRaises(ValueError):
            self.fake_battery.set(level=-5.0)

        with self.assertRaises(ValueError):
            self.fake_battery.set(level=105.0)

    def test_set_failure(self) -> None:
        """Test set raises FakeBatteryCommandError when command fails."""
        self.ffx_obj.run.side_effect = ffx_errors.FfxCommandError(
            "Command failed"
        )
        with self.assertRaises(fake_battery_errors.FakeBatteryCommandError):
            self.fake_battery.set(level=50.0)

    def test_set_level(self) -> None:
        """Test set_level convenience helper."""
        self.ffx_obj.run.return_value = "Success"
        self.fake_battery.set_level(88.0)
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                "fake-battery-cli set --level 88.0",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_set_charging(self) -> None:
        """Test set_charging convenience helper."""
        self.ffx_obj.run.return_value = "Success"
        self.fake_battery.set_charging(
            source=PowerSourceType.USB,
            voltage_mv=5000,
            current_ua=1500000,
        )
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                "fake-battery-cli set --status charging --source usb --voltage-mv 5000 --current-ua 1500000",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_set_discharging(self) -> None:
        """Test set_discharging convenience helper converts positive current to negative."""
        self.ffx_obj.run.return_value = "Success"
        self.fake_battery.set_discharging(
            current_ua=800000,
            voltage_mv=3700,
        )
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                "fake-battery-cli set --status discharging --source none --voltage-mv 3700 --current-ua -800000",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_set_health(self) -> None:
        """Test set_health convenience helper."""
        self.ffx_obj.run.return_value = "Success"
        self.fake_battery.set_health(HealthStatus.OVER_VOLTAGE)
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                "fake-battery-cli set --health over_voltage",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )

    def test_set_temperature(self) -> None:
        """Test set_temperature convenience helper."""
        self.ffx_obj.run.return_value = "Success"
        self.fake_battery.set_temperature(45000)
        self.ffx_obj.run.assert_called_with(
            [
                "component",
                "explore",
                "bootstrap/boot-drivers:dev.sys.platform.fake-battery",
                "--tools",
                _TOOL_URL,
                "-c",
                "fake-battery-cli set --temp-mc 45000",
            ],
            machine=ffx_types.MachineFormat.RAW,
        )


if __name__ == "__main__":
    unittest.main()
