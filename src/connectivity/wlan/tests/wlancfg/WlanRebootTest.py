#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import logging
import os
import time
from dataclasses import dataclass
from datetime import timedelta
from enum import Enum, StrEnum, auto, unique

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_wlan_base_test
from antlion import utils
from antlion.controllers import pdu
from antlion.controllers.ap_lib.hostapd_ap_preset import create_ap_preset
from antlion.controllers.ap_lib.hostapd_constants import AP_SSID_LENGTH_2G
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.radvd_config import RadvdConfig
from honeydew.affordances.connectivity.netstack.errors import (
    HoneydewNetstackError,
)
from honeydew.affordances.connectivity.netstack.types import PortClass
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from mobly import signals, test_runner
from openwrt_access_point import AddrType as OpenWrtAddrType
from openwrt_access_point import InterfaceName as OpenWrtInterfaceName
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    Band,
    BssSettings,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWpa2,
    SecurityWpa3,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

DUT_NETWORK_CONNECTION_TIMEOUT = timedelta(seconds=60)


@unique
class DeviceType(StrEnum):
    AP = auto()
    DUT = auto()


@unique
class RebootType(StrEnum):
    SOFT = auto()
    HARD = auto()


@unique
class IpVersionType(Enum):
    IPV4 = auto()
    IPV6 = auto()
    DUAL_IPV4_IPV6 = auto()

    def ipv4(self) -> bool:
        match self:
            case IpVersionType.IPV4:
                return True
            case IpVersionType.IPV6:
                return False
            case IpVersionType.DUAL_IPV4_IPV6:
                return True

    def ipv6(self) -> bool:
        match self:
            case IpVersionType.IPV4:
                return False
            case IpVersionType.IPV6:
                return True
            case IpVersionType.DUAL_IPV4_IPV6:
                return True

    @staticmethod
    def all() -> list[IpVersionType]:
        return [
            IpVersionType.IPV4,
            IpVersionType.IPV6,
            IpVersionType.DUAL_IPV4_IPV6,
        ]


@dataclass
class TestParams:
    reboot_device: DeviceType
    reboot_type: RebootType
    band: Band
    security: Security
    ip_version: IpVersionType


class WlanRebootTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    """Tests wlan reconnects in different reboot scenarios.

    Testbed Requirement:
    * One ACTS compatible device (dut)
    * One Whirlwind Access Point
    * One PduDevice
    """

    async def pre_run(self) -> None:
        test_params: list[tuple[TestParams]] = []

        # AP reboot wipes all AP state; the DUT's recovery flow is the same for
        # every security mode, so one representative is enough.
        for reboot_type in (RebootType.SOFT, RebootType.HARD):
            test_params.append(
                (
                    TestParams(
                        DeviceType.AP,
                        reboot_type,
                        Band.BAND_2G,
                        SecurityWpa2(),
                        IpVersionType.DUAL_IPV4_IPV6,
                    ),
                )
            )

        # DUT reboot checks that wlancfg reloads credentials from disk (PSK and
        # SAE) and reassociates with an AP that may hold stale PMF state.
        dut_securities: list[Security] = [SecurityWpa2(), SecurityWpa3()]
        for reboot_type in (RebootType.SOFT, RebootType.HARD):
            for security in dut_securities:
                for ip_version in IpVersionType.all():
                    test_params.append(
                        (
                            TestParams(
                                DeviceType.DUT,
                                reboot_type,
                                Band.BAND_2G,
                                security,
                                ip_version,
                            ),
                        )
                    )

        def generate_test_name(params: TestParams) -> str:
            # Map OpenWrt security to hostapd security string to match legacy test name format
            security = ConfigMapper.to_hostapd_security(params.security)
            test_name = (
                "test"
                f"_{params.reboot_type}_reboot"
                f"_{params.reboot_device}"
                f"_{params.band.lower()}"
                f"_{security}"
            )
            if params.ip_version.ipv4():
                test_name += "_ipv4"
            if params.ip_version.ipv6():
                test_name += "_ipv6"
            return test_name

        self.generate_tests(
            test_logic=self.run_reboot_test,
            name_func=generate_test_name,
            arg_sets=test_params,
        )

    async def setup_class(self) -> None:
        await super().setup_class()
        self.log = logging.getLogger()
        await self.dut.wlan_policy.set_country_code(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

        self.pdu_devices = await self.register_controller(
            pdu,
            required=False,
        )

        if not self.openwrt_ap and not self.access_point:
            raise signals.TestAbortClass("Requires at least one access point")

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_policy.ensure_clean_state()

    async def teardown_test(self) -> None:
        if self.access_point:
            self.access_point.stop_all_aps()
        await self.dut.wlan_policy.ensure_clean_state()
        await super().teardown_test()

    def setup_ap(
        self,
        ssid: str,
        band: Band,
        ip_version: IpVersionType,
        security: Security,
        password: str | None = None,
    ) -> None:
        """Setup ap with basic config.

        Args:
            ssid: The ssid to setup on ap
            band: The type of band to set up the ap with ('2g' or '5g').
            ip_version: The type of ip to use (ipv4 or ipv6)
            security_mode: The type of security mode.
            password: The PSK or passphase.
        """
        if self.openwrt_ap:
            channel = (
                DEFAULT_2G_CHANNEL
                if band == Band.BAND_2G
                else DEFAULT_5G_CHANNEL
            )
            config = AccessPointConfig(
                radios=[
                    RadioConfig.generate(
                        channel=channel,
                        bss_settings=[
                            BssSettings(
                                ssid=ssid,
                                security=security,
                                password=password,
                            )
                        ],
                    )
                ]
            )
            self.openwrt_ap.configure_wifi(config)
        elif self.access_point:
            legacy_security_mode = ConfigMapper.to_hostapd_security(security)
            security_profile = DeprecatedSecurity(
                security_mode=legacy_security_mode, password=password
            )
            legacy_band = ConfigMapper.to_hostapd_band(band)
            self.access_point.start_ap(
                hostapd_config=create_ap_preset(
                    iface_wlan_2g=self.access_point.wlan_2g,
                    iface_wlan_5g=self.access_point.wlan_5g,
                    profile_name="whirlwind",
                    channel=legacy_band.default_channel(),
                    ssid=ssid,
                    security=security_profile,
                    # TODO(http://b/271628778): Remove ap_max_inactivity once
                    # Fuchsia respects 802.11w (PMF) comeback-time.
                    ap_max_inactivity=100 if band == Band.BAND_5G else None,
                ),
                radvd_config=RadvdConfig() if ip_version.ipv6() else None,
            )

        if ip_version.ipv4():
            if self.openwrt_ap:
                self.openwrt_ap.dhcp.start_dhcp()
        else:
            if self.openwrt_ap:
                self.openwrt_ap.dhcp.stop_dhcp()
            elif self.access_point:
                self.access_point.stop_dhcp()

        self.log.info(f"Network (SSID: {ssid}) is up.")

    async def ping_dut_to_ap(
        self,
        band: Band,
        ip_version: IpVersionType,
    ) -> None:
        """Validate the DUT is pingable."""
        if self.openwrt_ap:
            if ip_version == IpVersionType.IPV4:
                ap_address = self.openwrt_ap.get_addr(
                    OpenWrtInterfaceName.lan,
                    OpenWrtAddrType.ipv4_private,
                )
            elif ip_version == IpVersionType.IPV6:
                ap_address = self.openwrt_ap.get_addr(
                    OpenWrtInterfaceName.lan,
                    OpenWrtAddrType.ipv6_link_local,
                )
            else:
                raise TypeError(f"Invalid IP type: {ip_version}")
        else:
            assert self.access_point is not None
            if band == Band.BAND_2G:
                test_interface = self.access_point.wlan_2g
            elif band == Band.BAND_5G:
                test_interface = self.access_point.wlan_5g

            if ip_version == IpVersionType.IPV4:
                ap_address = utils.get_addr(
                    self.access_point.ssh, test_interface
                )
            elif ip_version == IpVersionType.IPV6:
                ap_address = utils.get_addr(
                    self.access_point.ssh,
                    test_interface,
                    addr_type="ipv6_link_local",
                )
            else:
                raise TypeError(f"Invalid IP type: {ip_version}")

        if ap_address:
            if ip_version == IpVersionType.IPV4:
                ping_result = await self.dut.netstack.ping(ap_address)
            else:
                wlan_interface = await self.dut.netstack.wait_for_interface(
                    PortClass.WLAN_CLIENT
                )
                ap_address = f"{ap_address}%{wlan_interface.name}"
                ping_result = await self.dut.netstack.ping(ap_address)
            if ping_result.any_pings_received:
                self.log.info("Ping was successful.")
            else:
                raise signals.TestFailure(
                    f"Ping was unsuccessful: {ping_result.raw_output}"
                )
        else:
            raise ConnectionError("Failed to retrieve AP's ping address.")

    def write_csv_time_to_reconnect(
        self,
        test_name: str,
        reconnect_success: bool,
        time_to_reconnect: float = 0.0,
    ) -> None:
        """Writes the time to reconnect to a csv file.
        Args:
            test_name: the name of the test case
            reconnect_success: whether the test successfully reconnected or not
            time_to_reconnect: the time from when the rebooted device came back
                up to when it reassociated (or 'FAIL'), if it failed to
                reconnect.
        """
        csv_file_name = os.path.join(self.log_path, "time_to_reconnect.csv")
        self.log.info(f"Writing to {csv_file_name}")
        with open(csv_file_name, "a") as csv_file:
            if reconnect_success:
                csv_file.write(f"{test_name},{time_to_reconnect}\n")
            else:
                csv_file.write(f"{test_name},'FAIL'\n")

    def log_and_continue(
        self,
        ssid: str,
        time_to_reconnect: float = 0.0,
        error: Exception | None = None,
    ) -> None:
        """Writes the time to reconnect to the csv file before continuing, used
        in stress tests runs.

        Args:
            time_to_reconnect: the time from when the rebooted device came back
                ip to when reassociation occurred.
            error: error message to log before continuing with the test
        """
        if error:
            self.log.info(
                f"Device failed to reconnect to network {ssid}. Error: {error}"
            )
            self.write_csv_time_to_reconnect(
                f"{self.current_test_info.name}", False
            )

        else:
            self.log.info(
                f"Device successfully reconnected to network {ssid} after "
                f"{time_to_reconnect} seconds."
            )
            self.write_csv_time_to_reconnect(
                f"{self.current_test_info.name}", True, time_to_reconnect
            )

    async def run_reboot_test(self, params: TestParams) -> None:
        """Runs a reboot test based on a given config.
            1. Setups up a network, associates the dut, and saves the network.
            2. Verifies the dut receives ip address(es).
            3. Verifies traffic between DUT and AP (ping)
            4. Reboots (hard or soft) the device (dut or ap).
                - If the ap was rebooted, setup the same network again.
            5. Wait for reassociation or timeout.
            6. If reassocation occurs:
                - Verifies the dut receives ip address(es).
                - Verifies traffic between DUT and AP (ping).
            7. Logs time to reconnect (or failure to reconnect)

        Args:
            params: TestParams dataclass containing the following values:
                reboot_device: the device to reboot either DUT or AP.
                reboot_type: how to reboot the reboot_device either hard or soft.
                band: band to setup either 2g or 5g
                security: security mode to set up either OPEN, WPA2, or WPA3.
                ip_version: the ip version (ipv4 or ipv6)
        """
        assert self.openwrt_ap is not None or self.access_point is not None

        ssid = AccessPointConfig.random_string(AP_SSID_LENGTH_2G)
        reboot_device: DeviceType = params.reboot_device
        reboot_type: RebootType = params.reboot_type
        band: Band = params.band
        ip_version: IpVersionType = params.ip_version
        security: Security = params.security
        password: str | None = None
        if not isinstance(security, SecurityOpen):
            password = AccessPointConfig.random_string()

        self.setup_ap(
            ssid,
            band,
            ip_version,
            security,
            password,
        )

        security_type = security.to_fidl_wlan_policy()
        await self.dut.wlan_policy.save_network(ssid, security_type, password)
        await self.dut.wlan_policy.connect(ssid, security_type)

        wlan_interface = await self.dut.netstack.wait_for_interface(
            PortClass.WLAN_CLIENT
        )
        if ip_version.ipv4():
            await self.dut.netstack.wait_for_ipv4_addr(
                wlan_interface.id_,
                timeout=DUT_NETWORK_CONNECTION_TIMEOUT,
            )
            await self.ping_dut_to_ap(band, IpVersionType.IPV4)
        if ip_version.ipv6():
            await self.dut.netstack.wait_for_ipv6_addr(
                wlan_interface.id_,
                timeout=DUT_NETWORK_CONNECTION_TIMEOUT,
            )
            await self.ping_dut_to_ap(band, IpVersionType.IPV6)

        # DUT reboots
        if reboot_device == DeviceType.DUT:
            # TODO(b/273923552): We take a snapshot here before reboot
            # because the persistence component does not make the inspect logs
            # available for 120 seconds. This helps for debugging issues where
            # we need previous state.
            await self.dut.snapshot(directory=self.test_case_path)
            if reboot_type == RebootType.SOFT:
                await self.dut.reboot()
            elif reboot_type == RebootType.HARD:
                await self.dut.power_cycle()

        # AP reboots
        elif reboot_device == DeviceType.AP:
            if reboot_type == RebootType.SOFT:
                self.log.info("Cleanly stopping ap.")
                if self.openwrt_ap:
                    self.openwrt_ap.stop_wifi()
                elif self.access_point:
                    self.access_point.stop_all_aps()
            elif reboot_type == RebootType.HARD:
                if self.openwrt_ap:
                    # TODO(b/520236968): Implement power cycling for OpenWrt AP.
                    raise NotImplementedError(
                        "OpenWrt AP power cycling is not supported. (b/520236968)"
                    )
                elif self.access_point:
                    self.access_point.hard_power_cycle(self.pdu_devices)
            self.log.info(
                f"Waiting for DUT to disconnect from {ssid} after AP reboot. Will retry for "
                f"{DUT_NETWORK_CONNECTION_TIMEOUT.total_seconds():.0f} seconds."
            )
            await self.dut.wlan_policy.wait_for_no_connections(
                timeout=DUT_NETWORK_CONNECTION_TIMEOUT,
            )
            self.setup_ap(ssid, band, ip_version, security, password)

        uptime = time.time()
        try:
            try:
                self.log.info(
                    f"Checking if DUT is connected to {ssid} network. Will retry for "
                    f"{DUT_NETWORK_CONNECTION_TIMEOUT.total_seconds():.0f} seconds."
                )
                await self.dut.wlan_policy.wait_for_network_state(
                    ssid,
                    f_wlan_policy.ConnectionState.CONNECTED,
                    timeout=DUT_NETWORK_CONNECTION_TIMEOUT,
                )
            except HoneydewWlanError as e:
                if reboot_device == DeviceType.DUT and isinstance(
                    security, SecurityWpa3
                ):
                    # TODO(http://b/271628778): Remove this try/except statement
                    # once Fuchsia respects 802.11w (PMF) comeback-time.
                    raise signals.TestSkip(
                        f"Received expected ConnectionError due to http://b/271628778: {e}"
                    )
                raise e
            time_to_reconnect = time.time() - uptime

            wlan_interface = await self.dut.netstack.wait_for_interface(
                PortClass.WLAN_CLIENT
            )
            if ip_version.ipv4():
                await self.dut.netstack.wait_for_ipv4_addr(
                    wlan_interface.id_,
                    timeout=DUT_NETWORK_CONNECTION_TIMEOUT,
                )
                await self.ping_dut_to_ap(band, IpVersionType.IPV4)
            if ip_version.ipv6():
                await self.dut.netstack.wait_for_ipv6_addr(
                    wlan_interface.id_,
                    timeout=DUT_NETWORK_CONNECTION_TIMEOUT,
                )
                await self.ping_dut_to_ap(band, IpVersionType.IPV6)

        except (
            ConnectionError,
            HoneydewNetstackError,
            HoneydewWlanError,
            signals.TestFailure,
        ) as err:
            self.log_and_continue(ssid, error=err)
            raise signals.TestFailure(
                f"Failed to reconnect to {ssid} after reboot."
            ) from err
        else:
            self.log_and_continue(ssid, time_to_reconnect=time_to_reconnect)


if __name__ == "__main__":
    test_runner.main()
