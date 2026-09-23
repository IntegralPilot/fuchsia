#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Battery End-to-End Test."""

import logging

import fuchsia_base_test
from honeydew.affordances.drivers.battery_manager import (
    BatterySpec,
    ChargeStatus,
    PowerSourceSpec,
)
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class BatteryTest(fuchsia_base_test.FuchsiaBaseTest):
    """Test battery and fake battery affordances."""

    async def setup_test(self) -> None:
        await super().setup_test()
        self.battery = self.dut.battery
        self.fake_battery = self.dut.fake_battery

    async def teardown_test(self) -> None:
        try:
            self.fake_battery.set_charging()
        except Exception as err:
            _LOGGER.warning(
                "Failed to reset fake battery state in teardown: %s", err
            )
        await super().teardown_test()

    async def test_battery_get_spec(self) -> None:
        """Verify reading battery specification via get_spec and checking source_spec."""
        _LOGGER.info("Calling get_spec on %s", self.dut.device_name)
        spec = await self.battery.get_spec()
        _LOGGER.info("Battery spec received: %s", spec)
        asserts.assert_is_not_none(spec, msg="Battery spec should not be None")
        asserts.assert_is_instance(
            spec,
            BatterySpec,
            f"Expected BatterySpec, got {type(spec)}",
        )

        _LOGGER.info(
            "Accessing source_spec from battery spec: %s", spec.source_spec
        )
        if spec.source_spec is not None:
            asserts.assert_is_instance(
                spec.source_spec,
                PowerSourceSpec,
                f"Expected PowerSourceSpec, got {type(spec.source_spec)}",
            )
            _LOGGER.info("Power source type: %s", spec.source_spec.type)

    async def test_get_status(self) -> None:
        """Verify reading battery telemetry status via get_status."""
        _LOGGER.info("Calling get_status on %s", self.dut.device_name)
        status = await self.battery.get_status()
        _LOGGER.info("Battery status received: %s", status)
        asserts.assert_is_not_none(
            status, msg="Battery status should not be None"
        )

    async def test_set_discharging_status(self) -> None:
        """Verify setting battery charge status to discharging via fake battery and verifying with get_status."""
        _LOGGER.info(
            "Setting fake battery charge status to discharging on %s",
            self.dut.device_name,
        )
        self.fake_battery.set_discharging()

        _LOGGER.info(
            "Verifying battery status via get_status on %s",
            self.dut.device_name,
        )
        status = await self.battery.get_status()
        _LOGGER.info("Battery status received: %s", status)
        asserts.assert_equal(
            status.charge_status,
            ChargeStatus.DISCHARGING,
            f"Expected battery charge status to be DISCHARGING, got {status.charge_status}",
        )

    async def test_battery_status_across_reboot(self) -> None:
        """Verify reading battery status before and after a device reboot."""
        _LOGGER.info(
            "Verifying initial battery status on %s", self.dut.device_name
        )
        status_before = await self.battery.get_status()
        _LOGGER.info("Battery status before reboot: %s", status_before)
        asserts.assert_is_not_none(
            status_before, msg="Battery status before reboot should not be None"
        )

        _LOGGER.info("Rebooting device %s...", self.dut.device_name)
        await self.dut.reboot()
        _LOGGER.info("Device reboot completed successfully")

        _LOGGER.info(
            "Verifying battery status after reboot on %s", self.dut.device_name
        )
        status_after = await self.battery.get_status()
        _LOGGER.info("Battery status after reboot: %s", status_after)
        asserts.assert_is_not_none(
            status_after, msg="Battery status after reboot should not be None"
        )
        asserts.assert_equal(
            status_before,
            status_after,
            f"Expected battery status to be equal before and after reboot, "
            f"before: {status_before}, after: {status_after}",
        )


if __name__ == "__main__":
    test_runner.main()
