#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import ipaddress
import re
from ipaddress import IPv4Address

import dhcp_testing
import fuchsia_wlan_base_test
from antlion.controllers.ap_lib import dhcp_config
from antlion.controllers.utils_lib.commands import ip
from mobly import asserts, signals, test_runner
from openwrt_access_point import DhcpConfig, Dnsmasq
from openwrt_access_point import InterfaceName as OpenWrtInterfaceName
from openwrt_access_point import Lan


class Dhcpv4DuplicateAddressTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
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
        self.extra_addresses: list[IPv4Address] = []
        self.ap_params = self.dhcp.setup_ap()
        if self.access_point:
            self.ap_ip_cmd = ip.LinuxIpCommand(self.access_point.ssh)

    async def teardown_test(self) -> None:
        for ip in self.extra_addresses:
            self._remove_ap_ipv4_address(ip)
        await self.dut.wlan_policy.ensure_clean_state()
        if self.access_point:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    def _add_ap_ipv4_address(self, ip: IPv4Address) -> None:
        """Adds an IPv4 address to the AP's LAN interface."""
        if self.openwrt_ap:
            self.openwrt_ap.ssh.run(
                f"ip addr add {ip}/{ip.max_prefixlen} dev {OpenWrtInterfaceName.lan}"
            )
        elif self.access_point:
            self.ap_ip_cmd.add_ipv4_address(
                self.ap_params.id,
                ipaddress.IPv4Interface(f"{ip}/{ip.max_prefixlen}"),
            )

    def _remove_ap_ipv4_address(self, ip: IPv4Address) -> None:
        """Removes an IPv4 address from the AP's LAN interface."""
        if self.openwrt_ap:
            try:
                self.openwrt_ap.ssh.run(
                    f"ip addr del {ip}/{ip.max_prefixlen} dev {OpenWrtInterfaceName.lan}"
                )
            except Exception:
                pass
        elif self.access_point:
            self.ap_ip_cmd.remove_ipv4_address(self.ap_params.id, ip)

    async def test_duplicate_address_assignment(self) -> None:
        """It's possible for a DHCP server to assign an address that already exists on the network.
        DHCP clients are expected to perform a "gratuitous ARP" of the to-be-assigned address, and
        refuse to assign that address. Clients should also recover by asking for a different
        address.
        """
        if self.openwrt_ap:
            # OpenWrt case: Limit pool and disable ping check
            pool_start = 100
            pool_limit = 5

            self.openwrt_ap.dhcp.start_dhcp(
                config=DhcpConfig(
                    lan=Lan(start=pool_start, limit=pool_limit),
                    dnsmasq=Dnsmasq(noping=True),
                )
            )

            network_addr = self.ap_params.network.network_address
            pool_ips = [
                network_addr + i
                for i in range(pool_start, pool_start + pool_limit)
            ]
            # Add each of the usable IPs as an alias for the router's interface, such that the router
            # will respond to any pings on it.
            for ip in pool_ips:
                self._add_ap_ipv4_address(ip)
                # Ensure we remove the address in self.teardown_test() even if the test fails
                self.extra_addresses.append(ip)

        elif self.access_point:
            # Modify subnet to hold fewer addresses.
            # A '/29' has 8 addresses (6 usable excluding router / broadcast)
            subnet = next(self.ap_params.network.subnets(new_prefix=29))
            subnet_conf = dhcp_config.Subnet(
                subnet=subnet,
                router=self.ap_params.ip,
                # When the DHCP server is considering dynamically allocating an IP address to a client,
                # it first sends an ICMP Echo request (a ping) to the address being assigned. It waits
                # for a second, and if no ICMP Echo response has been heard, it assigns the address.
                # If a response is heard, the lease is abandoned, and the server does not respond to
                # the client.
                # The ping-check configuration parameter can be used to control checking - if its value
                # is false, no ping check is done.
                additional_parameters={"ping-check": "false"},
            )
            dhcp_conf = dhcp_config.DhcpConfig(subnets=[subnet_conf])
            self.access_point.start_dhcp(dhcp_conf=dhcp_conf)
            # Add each of the usable IPs as an alias for the router's interface, such that the router
            # will respond to any pings on it.
            for ip in subnet.hosts():
                self.ap_ip_cmd.add_ipv4_address(
                    self.ap_params.id,
                    ipaddress.IPv4Interface(f"{ip}/{ip.max_prefixlen}"),
                )
                # Ensure we remove the address in self.teardown_test() even if the test fails
                self.extra_addresses.append(ip)

        security = self.ap_params.security.to_fidl_wlan_policy()
        await self.dut.wlan_policy.save_network(
            self.ap_params.ssid, security, self.ap_params.password
        )
        await self.dut.wlan_policy.connect(self.ap_params.ssid, security)
        with asserts.assert_raises(ConnectionError):
            await self.dhcp.get_device_ipv4_addr()

        dhcp_logs = self.dhcp.get_dhcp_logs()
        if dhcp_logs is None:
            raise signals.TestError(
                "DHCP logs not found; was the DHCP server started?"
            )

        if self.openwrt_ap:
            # In this test, all IPs in the pool are marked as in-use on the AP interface.
            # The client detects the conflict after receiving DHCPOFFER and ignores it,
            # so it never sends a DHCPREQUEST. Thus, only DISCOVER and OFFER are seen.
            expected_patterns = [
                r"DHCPDISCOVER",
                r"DHCPOFFER",
            ]
            unexpected_patterns = [
                r"DHCPREQUEST",
                r"DHCPACK",
            ]

            # Positive checks
            for pattern in expected_patterns:
                asserts.assert_true(
                    re.search(pattern, dhcp_logs),
                    f"Did not find expected message ({pattern}) in logs",
                )

            # Negative checks
            for pattern in unexpected_patterns:
                asserts.assert_false(
                    re.search(pattern, dhcp_logs),
                    f"Found unexpected message ({pattern}) in logs which should not be there!",
                )
        elif self.access_point:
            # Per spec, the flow should be:
            # Discover -> Offer -> Request -> Ack -> client optionally performs DAD
            expected_patterns = [
                r"DHCPDISCOVER from \S+",
                r"DHCPOFFER on [0-9.]+ to \S+",
                r"DHCPREQUEST for [0-9.]+",
                r"DHCPACK on [0-9.]+",
                r"DHCPDECLINE of [0-9.]+ from \S+ via .*: abandoned",
                r"Abandoning IP address [0-9.]+: declined",
            ]
            for expected_message in expected_patterns:
                asserts.assert_true(
                    re.search(expected_message, dhcp_logs),
                    f"Did not find expected message ({expected_message}) in dhcp logs: {dhcp_logs}\n",
                )

        # Remove each of the IP aliases.
        while self.extra_addresses:
            ip = self.extra_addresses.pop()
            self._remove_ap_ipv4_address(ip)

        # Now, we should get an address successfully
        ip = await self.dhcp.get_device_ipv4_addr()
        dhcp_logs = self.dhcp.get_dhcp_logs()
        if dhcp_logs is None:
            raise signals.TestError(
                "DHCP logs not found; was the DHCP server started?"
            )

        request_matches = len(
            re.findall(rf"DHCPREQUEST.*{re.escape(str(ip))}", dhcp_logs)
        )
        asserts.assert_true(
            request_matches >= 1,
            f"Incorrect count of DHCP Requests in logs: {dhcp_logs}\n",
        )

        ack_matches = len(
            re.findall(rf"DHCPACK.*{re.escape(str(ip))}", dhcp_logs)
        )
        asserts.assert_true(
            ack_matches >= 1,
            f"Incorrect count of DHCP Acks in logs: {dhcp_logs}\n",
        )


if __name__ == "__main__":
    test_runner.main()
