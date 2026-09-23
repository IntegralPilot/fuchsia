# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging

import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from mobly import test_runner
from mobly.asserts import assert_equal

logger = logging.getLogger(__name__)


class PhyExistenceTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()

    async def test_ensure_single_phy(self) -> None:
        phy = await self.dut.wlan_core.ensure_single_phy()
        assert_equal(
            phy.id,
            self.phy.id,
            "WlanCore.ensure_single_phy() should return valid phy.",
        )


if __name__ == "__main__":
    test_runner.main()
