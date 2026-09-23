#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Learned battery parameters end-to-end test for MAX77779 PMIC."""

import logging
from typing import Any

import fuchsia_base_test
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

_MAX77779_SELECTOR: str = "*max77779*:root"


class LearnedBatteryParametersTest(fuchsia_base_test.FuchsiaBaseTest):
    """Verify learned battery parameters via MAX77779 PMIC Inspect telemetry."""

    async def test_learned_battery_parameters(self) -> None:
        """Verify cycle_count and full_charge_capacity in max77779 inspect data."""
        _LOGGER.info(
            "Querying Inspect data for MAX77779 on %s", self.dut.device_name
        )

        inspect_data_collection = self.dut.get_inspect_data(
            selectors=[_MAX77779_SELECTOR],
        )

        asserts.assert_true(
            bool(inspect_data_collection.data),
            f"No inspect data returned for selector '{_MAX77779_SELECTOR}' on {self.dut.device_name}",
        )

        # Find max77779 entry
        max77779_data = None
        for data in inspect_data_collection.data:
            if "max77779" in data.moniker:
                max77779_data = data
                break

        asserts.assert_is_not_none(
            max77779_data,
            f"Could not find max77779 inspect entry in {[d.moniker for d in inspect_data_collection.data]}",
        )
        assert max77779_data is not None
        asserts.assert_is_not_none(
            max77779_data.payload,
            f"Inspect payload is None for moniker {max77779_data.moniker}",
        )
        assert max77779_data.payload is not None

        _LOGGER.info(
            "Inspect payload for %s: %s",
            max77779_data.moniker,
            max77779_data.payload,
        )

        root = max77779_data.payload.get("root", {})
        asserts.assert_in(
            "fuel_gauge_subsystem",
            root,
            f"'fuel_gauge_subsystem' node missing in inspect root: {list(root.keys())}",
        )
        fg_subsystem: dict[str, Any] = root["fuel_gauge_subsystem"]

        # Validate cycle count presence
        # It can be under fuel_gauge_subsystem directly (charge_cycles/cycle_count),
        # in latest_status (charge_cycles), or learned_parameters (cycles).
        has_cycle_count = (
            "charge_cycles" in fg_subsystem
            or "cycle_count" in fg_subsystem
            or (
                "latest_status" in fg_subsystem
                and "charge_cycles" in fg_subsystem["latest_status"]
            )
            or (
                "learned_parameters" in fg_subsystem
                and "cycles" in fg_subsystem["learned_parameters"]
            )
        )
        asserts.assert_true(
            has_cycle_count,
            f"Expected cycle_count / charge_cycles in fuel_gauge_subsystem: {list(fg_subsystem.keys())}",
        )

        # Validate full charge capacity presence
        # It can be in fuel_gauge_subsystem / latest_status (full_cap_uah / full_charge_capacity),
        # or in learned_parameters (full_cap_nom / full_cap_rep).
        has_full_charge_capacity = (
            "full_cap_uah" in fg_subsystem
            or "full_charge_capacity" in fg_subsystem
            or (
                "latest_status" in fg_subsystem
                and "full_cap_uah" in fg_subsystem["latest_status"]
            )
            or (
                "learned_parameters" in fg_subsystem
                and (
                    "full_cap_nom" in fg_subsystem["learned_parameters"]
                    or "full_cap_rep" in fg_subsystem["learned_parameters"]
                )
            )
        )
        asserts.assert_true(
            has_full_charge_capacity,
            f"Expected full_charge_capacity / full_cap_uah / full_cap_nom in fuel_gauge_subsystem: {list(fg_subsystem.keys())}",
        )

        _LOGGER.info(
            "Learned battery parameters successfully validated in Inspect data."
        )


if __name__ == "__main__":
    test_runner.main()
