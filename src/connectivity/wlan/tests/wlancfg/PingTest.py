#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
PingTest exercises sending ICMP and ICMPv6 pings to a wireless access
router and another device behind the AP. Note, this does not reach out to the
internet. The DUT is only responsible for sending a routable packet; any
communication past the first-hop is not the responsibility of the DUT.
"""

import asyncio
import logging
from datetime import timedelta
from typing import Callable, NamedTuple

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion import utils
from antlion.controllers.access_point import setup_ap
from honeydew.affordances.connectivity.netstack.types import (
    PingResult,
    PortClass,
)
from mobly import asserts, signals, test_runner
from openwrt_access_point import AddrType as OpenWrtAddrType
from openwrt_access_point import InterfaceName as OpenWrtInterfaceName
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    Band,
    BssSettings,
    RadioConfig,
    SecurityOpen,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)


class Addrs(NamedTuple):
    gateway_ipv4: str
    gateway_ipv6: str


class PingParams(NamedTuple):
    name: str
    dest_ip: str | Callable[[Addrs], str]
    packet_count: int = 3
    interval: timedelta = timedelta(seconds=0.1)
    timeout: timedelta = timedelta(seconds=1)
    size: int = 25
    min_success: int | None = None


class PingTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    async def pre_run(self) -> None:
        tests = [
            PingParams(
                "gateway_ipv4_small_packets",
                lambda addrs: addrs.gateway_ipv4,
                packet_count=50,
                min_success=49,
            ),
            PingParams(
                "gateway_ipv6_small_packets",
                lambda addrs: addrs.gateway_ipv6,
                packet_count=50,
                min_success=49,
            ),
            PingParams(
                "gateway_ipv4_medium_packets",
                lambda addrs: addrs.gateway_ipv4,
                packet_count=50,
                timeout=timedelta(milliseconds=1500),
                size=64,
                min_success=49,
            ),
            PingParams(
                "gateway_ipv6_medium_packets",
                lambda addrs: addrs.gateway_ipv6,
                packet_count=50,
                timeout=timedelta(milliseconds=1500),
                size=64,
                min_success=49,
            ),
            PingParams(
                "gateway_ipv4_large_packets",
                lambda addrs: addrs.gateway_ipv4,
                packet_count=50,
                timeout=timedelta(seconds=5),
                size=500,
                min_success=49,
            ),
            PingParams(
                "gateway_ipv6_large_packets",
                lambda addrs: addrs.gateway_ipv6,
                packet_count=50,
                timeout=timedelta(seconds=5),
                size=500,
                min_success=49,
            ),
        ]
        self.generate_tests(
            self.send_ping,
            lambda param: f"test_{param.name}",
            [(t,) for t in tests],
        )

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()
        self.ssid = AccessPointConfig.random_string(10)

        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

        await self.dut.wlan_policy.ensure_clean_state()

        band = Band.BAND_2G
        security = SecurityOpen()

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=self.ssid,
                                security=security,
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)

            # Retrieve Gateway IPs.
            self.ap_ipv4 = self.openwrt_ap.get_addr(
                OpenWrtInterfaceName.lan,
                OpenWrtAddrType.ipv4_private,
            )
            self.ap_ipv6 = self.openwrt_ap.get_addr(
                OpenWrtInterfaceName.lan,
                OpenWrtAddrType.ipv6_link_local,
            )
        else:
            assert self.access_point is not None
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=ConfigMapper.to_hostapd_band(band).default_channel(),
                ssid=self.ssid,
                setup_bridge=True,
                is_ipv6_enabled=True,
                is_nat_enabled=False,
            )

            ap_bridges = self.access_point.interfaces.get_bridge_interface()
            if not ap_bridges:
                raise signals.TestAbortClass(
                    f"Expected bridge interfaces on the AP, got {ap_bridges}"
                )
            ap_bridge = ap_bridges[0]
            self.ap_ipv4 = utils.get_addr(self.access_point.ssh, ap_bridge)
            self.ap_ipv6 = utils.get_addr(
                self.access_point.ssh, ap_bridge, addr_type="ipv6_link_local"
            )

        self.log.info(
            f"Gateway finished setup ({self.ap_ipv4} | {self.ap_ipv6})"
        )
        await self.dut.wlan_policy.save_network(
            self.ssid, f_wlan_policy.SecurityType.NONE
        )
        await self.dut.wlan_policy.connect(
            self.ssid, f_wlan_policy.SecurityType.NONE
        )
        self.wlan_interface = await self.wait_for_interface(
            self.dut.netstack, PortClass.WLAN_CLIENT
        )

    async def teardown_class(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_class()

    async def send_ping(self, param: PingParams) -> None:
        dest_ip = (
            param.dest_ip(
                Addrs(
                    gateway_ipv4=self.ap_ipv4,
                    # IPv6 link-local addresses require specification of the
                    # outgoing interface as the scope ID when sending packets.
                    gateway_ipv6=f"{self.ap_ipv6}%{self.wlan_interface}",
                )
            )
            if callable(param.dest_ip)
            else param.dest_ip
        )

        self.log.info(f"Attempting to ping {dest_ip}...")
        ping_result = await self.dut.netstack.ping(
            dest_ip,
            count=param.packet_count,
            interval=param.interval,
            timeout=param.timeout,
            size=param.size,
        )
        min_success = param.min_success or param.packet_count
        if not ping_result.any_pings_received:
            raise signals.TestFailure(
                f"Failed to ping {dest_ip}: {ping_result.raw_output}"
            )
        asserts.assert_greater_equal(
            ping_result.received,
            min_success,
            f"Expected at least {min_success}/{param.packet_count} packets received, but got {ping_result.received}/{param.packet_count}",
        )
        self.log.info(
            f"Ping test to {dest_ip} passed ({ping_result.received}/{param.packet_count})"
        )

    async def test_simultaneous_pings(self) -> None:
        ping_urls = [
            self.ap_ipv4,
            f"{self.ap_ipv6}%{self.wlan_interface}",
        ]

        async def ping_from_dut(dest_ip: str) -> PingResult:
            self.log.info(f"Attempting to ping {dest_ip}...")
            ping_result = await self.dut.netstack.ping(
                dest_ip, count=10, size=50
            )
            if ping_result.any_pings_received:
                self.log.info(f"Success pinging: {dest_ip}")
            else:
                self.log.info(f"Failure pinging: {dest_ip}")
            return ping_result

        self.log.info("Starting simultaneous pings...")
        results = await asyncio.gather(
            *[ping_from_dut(url) for url in ping_urls]
        )

        for i, ping_result in enumerate(results):
            if not ping_result.any_pings_received:
                raise signals.TestFailure(
                    f"Failed to ping {ping_urls[i]}: {ping_result.raw_output}"
                )


if __name__ == "__main__":
    test_runner.main()
