# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import logging

import fidl_fuchsia_wlan_internal as f_wlan_internal
import fidl_fuchsia_wlan_sme as f_wlan_sme
import fuchsia_base_test
import openwrt_access_point
from honeydew.affordances.connectivity.wlan.utils import errors as wlan_errors
from mobly import asserts, signals, test_runner
from openwrt_access_point import OpenWrtAP
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    SecurityOpen,
    SecurityWpa2,
    SecurityWpa2Wpa3Mixed,
    SecurityWpa3,
)

_LOGGER: logging.Logger = logging.getLogger(__name__)


class OpenWrtAPScanConnectTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_class(self) -> None:
        """setup_class is called once before running tests."""
        await super().setup_class()
        self.log = logging.getLogger()

        if not self.fuchsia_devices:
            raise signals.TestAbortClass(
                "At least one Fuchsia device is required"
            )
        self.openwrt_aps: list[
            OpenWrtAP
        ] | None = await self.register_controller(openwrt_access_point)
        if not self.fuchsia_devices:
            raise signals.TestAbortClass(
                "At least one Fuchsia device is required"
            )
        if not self.openwrt_aps:
            raise signals.TestAbortClass(
                "At least one OpenWRT access point is required"
            )

        self.openwrt_ap = self.openwrt_aps[0]

    async def _test_scan_and_connect(
        self, wifi_config: AccessPointConfig
    ) -> None:
        """Helper to test scanning and connecting for a given Wi-Fi config."""
        assert (
            wifi_config.radios
        ), "Expected at least one Radio configuration, but found None."
        assert wifi_config.radios[
            0
        ].bss_settings, (
            "Expected at least one BSS configuration, but found None."
        )
        bss_settings = wifi_config.radios[0].bss_settings[0]

        self.openwrt_ap.configure_wifi(wifi_config)

        phy = await self.dut.wlan_core.ensure_single_phy()
        iface = await phy.create_client_iface()

        # TODO: https://fxbug.dev/487800358 - Create and use to_fidl() function.
        if isinstance(bss_settings.security, SecurityWpa2):
            if not bss_settings.password:
                raise signals.TestFailure(
                    "Password must be provided for WPA2 security"
                )
            security_protocol = f_wlan_internal.Protocol.WPA2_PERSONAL
        elif isinstance(
            bss_settings.security, (SecurityWpa3, SecurityWpa2Wpa3Mixed)
        ):
            if not bss_settings.password:
                raise signals.TestFailure(
                    "Password must be provided for WPA3 security"
                )
            security_protocol = f_wlan_internal.Protocol.WPA3_PERSONAL
        else:
            security_protocol = f_wlan_internal.Protocol.OPEN

        self.log.info(
            "Starting scan and connect for SSID: %s", bss_settings.ssid
        )
        success = False
        for attempt in range(3):  # Retry up to 3 times
            try:
                await iface.scan_and_connect(
                    ssid=bss_settings.ssid,
                    password=bss_settings.password,
                    security=security_protocol,
                )
                success = True
                break
            except wlan_errors.HoneydewWlanError as e:
                self.log.info(
                    f"SSID {bss_settings.ssid} failed scan and connect on attempt {attempt + 1}: {e}, retrying..."
                )
                await asyncio.sleep(2)

        asserts.assert_true(success, "Failed to connect.")

        self.log.info("Connected to SSID: %s", bss_settings.ssid)

        await iface.disconnect()
        status = await iface.status()
        if status != f_wlan_sme.ClientStatusResponse(idle=f_wlan_sme.Empty()):
            asserts.fail(
                f"Status did not return to idle after disconnect: {status}"
            )
        await iface.destroy()

    async def test_scan_and_connect_2g(self) -> None:
        """Test case for scanning and connecting to a 2G OpenWRT AP."""
        await self._test_scan_and_connect(
            AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=AccessPointConfig.random_string(),
                                security=SecurityOpen(),
                            )
                        ],
                    )
                ]
            )
        )

    async def test_scan_and_connect_5g(self) -> None:
        """Test case for scanning and connecting to a 5G OpenWRT AP."""
        await self._test_scan_and_connect(
            AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_5G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=AccessPointConfig.random_string(),
                                security=SecurityOpen(),
                            )
                        ],
                    )
                ]
            )
        )

    async def test_scan_and_connect_wpa2(self) -> None:
        """Test case for scanning and connecting to a WPA2 network."""
        await self._test_scan_and_connect(
            AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=AccessPointConfig.random_string(),
                                security=SecurityWpa2(),
                                password=AccessPointConfig.random_string(16),
                            )
                        ],
                    )
                ]
            )
        )

    async def test_scan_and_connect_wpa3(self) -> None:
        """Test case for scanning and connecting to a WPA3 network."""
        await self._test_scan_and_connect(
            AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=AccessPointConfig.random_string(),
                                security=SecurityWpa2Wpa3Mixed(),
                                password=AccessPointConfig.random_string(16),
                            )
                        ],
                    )
                ]
            )
        )


if __name__ == "__main__":
    test_runner.main()
