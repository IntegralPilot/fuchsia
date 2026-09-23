# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import re
from dataclasses import dataclass
from datetime import timedelta
from ipaddress import IPv4Address, IPv4Network
from pathlib import Path

from antlion.controllers.access_point import (
    AccessPoint,
    setup_ap,
)
from antlion.controllers.ap_lib import dhcp_config, hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode as DeprecatedSecurityMode,
)
from honeydew.affordances.connectivity.netstack.errors import (
    HoneydewNetstackError,
)
from honeydew.affordances.connectivity.netstack.types import PortClass
from honeydew.fuchsia_device import fuchsia_device
from mobly import asserts, signals
from openwrt_access_point import AddrType as OpenWrtAddrType
from openwrt_access_point import InterfaceName as OpenWrtInterfaceName
from openwrt_access_point import OpenWrtAP
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    Security,
    SecurityWpa2,
)

_LOGGER = logging.getLogger(__name__)


@dataclass
class APParams:
    id: str
    ssid: str
    security: Security
    password: str
    ip: IPv4Address
    network: IPv4Network


class DhcpHelper:
    """Helper for DHCPv4 testing that encapsulates DUT and AP controllers."""

    def __init__(
        self,
        dut: fuchsia_device.FuchsiaDevice,
        openwrt_ap: OpenWrtAP | None = None,
        access_point: AccessPoint | None = None,
        log_path: str = "",
    ) -> None:
        self.dut = dut
        self.openwrt_ap = openwrt_ap
        self.access_point = access_point
        self.log_path = log_path
        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

    def setup_ap(
        self,
    ) -> APParams:
        """Generates an AP config and sets up the AP with that config.

        Does not run a DHCP server.

        Returns:
            APParams for the newly setup AP.
        """
        ssid = AccessPointConfig.random_string(20)
        password = AccessPointConfig.random_string(20)

        if self.openwrt_ap:
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=DEFAULT_2G_CHANNEL,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=SecurityWpa2(),
                                password=password,
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)

            router_ip = IPv4Address(
                self.openwrt_ap.get_addr(
                    interface=OpenWrtInterfaceName.lan,
                    addr_type=OpenWrtAddrType.ipv4_private,
                )
            )
            network = IPv4Network(f"{router_ip}/24", strict=False)

            self.openwrt_ap.dhcp.stop_dhcp()

            return APParams(
                id="radio0",
                ssid=ssid,
                security=SecurityWpa2(),
                password=password,
                ip=router_ip,
                network=network,
            )
        elif self.access_point:
            security = DeprecatedSecurity(
                security_mode=DeprecatedSecurityMode.WPA2,
                password=password,
                wpa_cipher="CCMP",
                wpa2_cipher="CCMP",
            )

            ap_ids = setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                mode=hostapd_constants.Mode.MODE_11N_MIXED,
                channel=hostapd_constants.AP_DEFAULT_CHANNEL_2G,
                n_capabilities=[],
                ac_capabilities=[],
                force_wmm=True,
                ssid=ssid,
                security=security,
            )

            if len(ap_ids) > 1:
                raise Exception("Expected only one SSID on AP")

            configured_subnets = self.access_point.get_configured_subnets()
            if len(configured_subnets) > 1:
                raise Exception("Expected only one subnet on AP")
            router_ip = configured_subnets[0].router
            network = configured_subnets[0].network

            self.access_point.stop_dhcp()

            return APParams(
                id=ap_ids[0],
                ssid=ssid,
                security=SecurityWpa2(),
                password=password,
                ip=router_ip,
                network=network,
            )
        else:
            raise signals.TestAbortClass("Requires at least one access point")

    async def get_device_ipv4_addr(
        self,
        timeout: timedelta = timedelta(seconds=30),
    ) -> IPv4Address:
        """Checks if device has an ipv4 address on the WLAN client interface.

        Raises:
            ConnectionError: if DUT does not have an ipv4 address after timeout.

        Returns:
            The device's IP address.
        """
        try:
            wlan_iface = await self.dut.netstack.wait_for_interface(
                port_class=PortClass.WLAN_CLIENT,
                timeout=timeout,
            )
            _LOGGER.info(
                "Acquired WLAN client interface ID %s. Waiting for IPv4 address...",
                wlan_iface.id_,
            )
            ip = await self.dut.netstack.wait_for_ipv4_addr(
                interface_id=wlan_iface.id_,
                timeout=timeout,
            )
            _LOGGER.info("DUT has an ipv4 address: %s", ip)
            return ip
        except HoneydewNetstackError as e:
            raise ConnectionError("DUT failed to get an ipv4 address.") from e

    def get_dhcp_logs(self) -> str:
        """Fetches DHCP server logs from the access point."""
        if self.openwrt_ap:
            val = self.openwrt_ap.dhcp.get_dhcp_logs_since_last_dhcp_start()
            assert isinstance(val, str)
            return val
        elif self.access_point:
            dhcp_logs = self.access_point.get_dhcp_logs()
            if dhcp_logs is None:
                raise signals.TestFailure("No DHCP logs")
            assert isinstance(dhcp_logs, str)
            return dhcp_logs
        else:
            raise signals.TestFailure("No access point found")

    async def run_test_case_expect_dhcp_success(
        self,
        dhcp_parameters: dict[str, str] | None = None,
        dhcp_options: dict[str, int | str] | None = None,
    ) -> None:
        """Starts the AP and DHCP server, and validates that the client
        connects and obtains an address.

        Args:
            dhcp_parameters: a dictionary of DHCP parameters.
            dhcp_options: a dictionary of DHCP options.
        """
        if dhcp_parameters is None:
            dhcp_parameters = {}
        if dhcp_options is None:
            dhcp_options = {}

        ap_params = self.setup_ap()
        subnet_conf = dhcp_config.Subnet(
            subnet=ap_params.network,
            router=ap_params.ip,
            additional_parameters=dhcp_parameters,
            additional_options=dhcp_options,
        )
        dhcp_conf = dhcp_config.DhcpConfig(subnets=[subnet_conf])

        _LOGGER.debug(
            "DHCP Configuration:\n%s\n", dhcp_conf.render_config_file()
        )

        security = ap_params.security.to_fidl_wlan_policy()
        if self.openwrt_ap:
            self.openwrt_ap.dhcp.start_dhcp()
            await self.dut.wlan_policy.save_network(
                ap_params.ssid, security, ap_params.password
            )
            await self.dut.wlan_policy.connect(ap_params.ssid, security)

            try:
                ip = await self.get_device_ipv4_addr()
            except ConnectionError:
                _LOGGER.warning(
                    "DHCP logs: %s",
                    self.openwrt_ap.dhcp.get_dhcp_logs_since_last_dhcp_start(),
                )
                raise signals.TestFailure("DUT failed to get an IP address")

        elif self.access_point:
            with self.access_point.tcpdump.start(
                self.access_point.wlan_2g, Path(self.log_path)
            ):
                self.access_point.start_dhcp(dhcp_conf=dhcp_conf)
                await self.dut.wlan_policy.save_network(
                    ap_params.ssid, security, ap_params.password
                )
                await self.dut.wlan_policy.connect(ap_params.ssid, security)

                try:
                    ip = await self.get_device_ipv4_addr()
                except ConnectionError:
                    _LOGGER.warning(
                        "DHCP logs: %s", self.access_point.get_dhcp_logs()
                    )
                    raise signals.TestFailure("DUT failed to get an IP address")
        else:
            raise signals.TestAbortClass("Requires at least one access point")

        dhcp_logs = self.get_dhcp_logs()
        discover_count = dhcp_logs.count("DHCPDISCOVER")
        offer_count = len(
            re.findall(rf"DHCPOFFER.*{re.escape(str(ip))}", dhcp_logs)
        )
        asserts.assert_greater(
            discover_count,
            0,
            "Expected one or more DHCP Discovers",
            dhcp_logs,
        )
        asserts.assert_equal(
            discover_count,
            offer_count,
            "Expected an equal amount of DHCP Discovers and Offers",
            dhcp_logs,
        )

        request_count = len(
            re.findall(rf"DHCPREQUEST.*{re.escape(str(ip))}", dhcp_logs)
        )
        asserts.assert_true(
            request_count >= 1,
            f"Incorrect count of DHCP Requests in logs:\n{dhcp_logs}\n",
        )
        ack_count = len(
            re.findall(rf"DHCPACK.*{re.escape(str(ip))}", dhcp_logs)
        )
        asserts.assert_true(
            ack_count >= 1,
            f"Incorrect count of DHCP Acks in logs:\n{dhcp_logs}\n",
        )

        _LOGGER.info("Attempting to ping %s...", ap_params.ip)
        ping_result = await self.dut.netstack.ping(str(ap_params.ip), count=2)
        asserts.assert_true(
            ping_result.any_pings_received,
            f"DUT failed to ping router at {ap_params.ip}: {ping_result.raw_output}",
        )
