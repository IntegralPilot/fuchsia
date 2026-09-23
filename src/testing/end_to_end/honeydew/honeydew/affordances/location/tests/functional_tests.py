# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for location affordance."""

import logging

import fuchsia_base_test
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
    CountryCode,
)
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

TIMEOUT_COUNTRY_CODE_SEC = 10.0
"""Seconds to wait for the country code to propagate to WLAN PHYs."""


class LocationTests(fuchsia_base_test.FuchsiaBaseTest):
    """Location affordance tests"""

    async def test_set_region(self) -> None:
        """Verify set_region() works on device."""
        await self.dut.location.set_region(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

    async def test_set_region_fails(self) -> None:
        """Verify set_region() fails on device with incorrect args."""

        # Do not expect an error. wlancfg's regulatory_manager does not
        # propagate an error up to RegulatoryRegionConfigurator when setting an
        # invalid country code.
        #
        # TODO(http://b/370600007): Replace with assert_raises once there is
        # error checking in set_region.
        await self.dut.location.set_region(CountryCode("??"))


if __name__ == "__main__":
    test_runner.main()
