#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import logging
import re

import dhcp_testing
import fuchsia_wlan_base_test
from antlion.controllers.ap_lib import dhcp_config
from mobly import asserts, test_runner
from openwrt_access_point.lib.dhcp_config import DhcpConfig, Lan

_LOGGER = logging.getLogger(__name__)


class Dhcpv4InteropBasicTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """DhcpV4 tests which validate basic DHCP client/server interactions."""

    async def setup_class(self) -> None:
        await super().setup_class()
        self.dhcp = dhcp_testing.DhcpHelper(
            dut=self.dut,
            openwrt_ap=self.openwrt_ap,
            access_point=self.access_point,
            log_path=self.log_path,
        )

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def test_basic_dhcp_assignment(self) -> None:
        await self.dhcp.run_test_case_expect_dhcp_success(
            dhcp_options={},
            dhcp_parameters={},
        )

    async def test_pool_allows_unknown_clients(self) -> None:
        await self.dhcp.run_test_case_expect_dhcp_success(
            dhcp_options={},
            dhcp_parameters={"allow": "unknown-clients"},
        )

    async def test_pool_disallows_unknown_clients(self) -> None:
        ap_params = self.dhcp.setup_ap()

        if self.openwrt_ap:
            self.openwrt_ap.dhcp.start_dhcp(
                config=DhcpConfig(lan=Lan(dynamic_dhcp=False))
            )
        elif self.access_point:
            subnet_conf = dhcp_config.Subnet(
                subnet=ap_params.network,
                router=ap_params.ip,
                additional_parameters={"deny": "unknown-clients"},
            )
            dhcp_conf = dhcp_config.DhcpConfig(subnets=[subnet_conf])
            self.access_point.start_dhcp(dhcp_conf=dhcp_conf)

        security = ap_params.security.to_fidl_wlan_policy()
        await self.dut.wlan_policy.save_network(
            ap_params.ssid, security, ap_params.password
        )
        await self.dut.wlan_policy.connect(ap_params.ssid, security)
        with asserts.assert_raises(ConnectionError):
            await self.dhcp.get_device_ipv4_addr()

        dhcp_logs = self.dhcp.get_dhcp_logs()

        pattern = ""
        if self.openwrt_ap:
            # dnsmasq logs "no address available" when dynamic DHCP is disabled and the client is unknown.
            pattern = r"DHCPDISCOVER.*no address available"
        elif self.access_point:
            # ISC DHCPD logs "no free leases" when it cannot offer a lease due to "deny unknown-clients".
            pattern = r"DHCPDISCOVER.*no free leases"

        asserts.assert_true(
            re.search(pattern, dhcp_logs),
            f"Did not find expected message in dhcp logs: {dhcp_logs}\n",
        )

    async def test_lease_renewal(self) -> None:
        """Validates that a client renews their DHCP lease."""
        ap_params = self.dhcp.setup_ap()
        LEASE_TIME = 30
        if self.openwrt_ap:
            # The min lease time is 2m for OpenWRT AP
            LEASE_TIME = 120
            self.openwrt_ap.dhcp.start_dhcp(
                config=DhcpConfig(lan=Lan(lease_time=f"{LEASE_TIME}s"))
            )
        elif self.access_point:
            subnet_conf = dhcp_config.Subnet(
                subnet=ap_params.network, router=ap_params.ip
            )
            dhcp_conf = dhcp_config.DhcpConfig(
                subnets=[subnet_conf],
                default_lease_time=LEASE_TIME,
                max_lease_time=LEASE_TIME,
            )
            self.access_point.start_dhcp(dhcp_conf=dhcp_conf)

        security = ap_params.security.to_fidl_wlan_policy()
        await self.dut.wlan_policy.save_network(
            ap_params.ssid, security, ap_params.password
        )
        await self.dut.wlan_policy.connect(ap_params.ssid, security)
        ip = await self.dhcp.get_device_ipv4_addr()

        SLEEP_TIME = LEASE_TIME + 3
        _LOGGER.info("Sleeping %ss to await DHCP renewal", SLEEP_TIME)
        await asyncio.sleep(SLEEP_TIME)

        dhcp_logs = self.dhcp.get_dhcp_logs()

        # Fuchsia renews at LEASE_TIME / 2, so there should be at least 2 DHCPREQUESTs in logs.
        # The log lines look like:
        # INFO dhcpd[17385]: DHCPREQUEST for 192.168.9.2 from 01:23:45:67:89:ab via wlan1
        # INFO dhcpd[17385]: DHCPACK on 192.168.9.2 to 01:23:45:67:89:ab via wlan1
        request_matches = len(
            re.findall(rf"DHCPREQUEST.*{re.escape(str(ip))}", dhcp_logs)
        )
        asserts.assert_true(
            request_matches >= 2,
            f"Not enough DHCP renewals in logs: {dhcp_logs}\n",
        )

    async def test_no_dhcp_server_started(self) -> None:
        """Validates that DUT fails to get an address when no DHCP server is running."""
        ap_params = self.dhcp.setup_ap()
        if self.access_point:
            # Allow Whirlwind AP radio to finish initializing and broadcasting beacons
            await asyncio.sleep(5)
        security = ap_params.security.to_fidl_wlan_policy()
        await self.dut.wlan_policy.save_network(
            ap_params.ssid, security, ap_params.password
        )
        await self.dut.wlan_policy.connect(ap_params.ssid, security)
        with asserts.assert_raises(ConnectionError):
            await self.dhcp.get_device_ipv4_addr()


if __name__ == "__main__":
    test_runner.main()
