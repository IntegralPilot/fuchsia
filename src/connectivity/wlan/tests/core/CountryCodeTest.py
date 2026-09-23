# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import asserts, test_runner

logger = logging.getLogger(__name__)


class CountryCodeTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

    async def test_phy_response_to_country_code_change(self) -> None:
        original_country_code = await self.phy.get_country()
        logger.info(f"Original country code is {original_country_code}")

        # Set a new country code
        new_country_code = (
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
            if original_country_code
            != KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
            else KNOWN_COUNTRY_CODES["JAPAN"]
        )
        logger.info(f"Setting country code to {new_country_code}...")
        await self.phy.set_country(new_country_code)

        current_country_code = await self.phy.get_country()
        asserts.assert_equal(
            current_country_code,
            new_country_code,
            f"Failed to set a new country code.",
        )

        # Do not restore the original country code. Tests that require
        # setting a particular country code should set the country
        # code themselves. Otherwise, tests should assume only
        # globally supported channels are available, and those
        # channels are available, by definition, regardless of the
        # country code setting.


if __name__ == "__main__":
    test_runner.main()
