# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Tests of various (mostly hardcoded) properties returned from a WifiChip.
"""

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
from mobly import test_runner
from mobly.asserts import assert_equal
from wlanix_testing import base_test


class WifiChipCorrectnessAndConsistencyTest(base_test.WifiChipBaseTestClass):
    async def test_chip_id_match(self) -> None:
        response = (await self.wifi_chip_proxy.get_id()).unwrap()
        assert_equal(
            response.id_,
            self.chip_id,
            "WifiChip itself returns a different id than chip_id",
        )

    # TODO(https://fxbug.dev/366028666): GetMode is hardcoded.
    async def test_chip_mode_zeroed(
        self,
    ) -> None:
        response = (await self.wifi_chip_proxy.get_mode()).unwrap()
        assert_equal(
            response.mode, 0, "WifiChip should have returned a hardcoded 0 mode"
        )

    # TODO(https://fxbug.dev/366027488): GetAvailableModes is hardcoded.
    async def test_chip_available_modes_as_hardcoded(
        self,
    ) -> None:
        response = (await self.wifi_chip_proxy.get_available_modes()).unwrap()
        assert_equal(
            response,
            fidl_wlanix.WifiChipGetAvailableModesResponse(
                chip_modes=[
                    fidl_wlanix.ChipMode(
                        id_=self.chip_id,
                        available_combinations=[
                            fidl_wlanix.ChipConcurrencyCombination(
                                limits=[
                                    fidl_wlanix.ChipConcurrencyCombinationLimit(
                                        types=[
                                            fidl_wlanix.IfaceConcurrencyType.STA
                                        ],
                                        max_ifaces=1,
                                    )
                                ]
                            )
                        ],
                    )
                ]
            ),
            "WifiChip returned a ChipMode different than the expected hardcoded response",
        )

    # TODO(https://fxbug.dev/366027491): GetCapabilities is hardcoded.
    async def test_chip_capabilities_zeroed(self) -> None:
        response = (await self.wifi_chip_proxy.get_capabilities()).unwrap()
        assert_equal(
            response.capabilities_mask,
            0,
            "WifiChip should have returned a hardcoded 0 capabilities_mask",
        )

    async def test_chip_iface_names_empty(self) -> None:
        response = (await self.wifi_chip_proxy.get_sta_iface_names()).unwrap()
        assert response.iface_names is not None
        assert_equal(
            len(response.iface_names),
            0,
            "WifiChip should have returned an empty list of iface names",
        )


if __name__ == "__main__":
    test_runner.main()
