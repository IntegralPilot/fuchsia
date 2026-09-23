#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import itertools
import logging
import time
from dataclasses import dataclass
from enum import StrEnum, auto, unique

from antlion.controllers.ap_lib.hostapd_ap_preset import create_ap_preset
from antlion.controllers.ap_lib.hostapd_security import (
    Security as HostapdSecurity,
)
from antlion.controllers.ap_lib.radvd_config import RadvdConfig
from antlion.controllers.attenuator import (
    Attenuator,
    get_attenuators_for_device,
)
from antlion.controllers.fuchsia_device import FuchsiaDevice
from antlion.controllers.iperf_server import IPerfResult
from antlion.utils import rand_ascii_str
from antlion.validation import MapValidator
from fuchsia_wlan_base_test.deprecated.wifi import base_test
from mobly import asserts, signals, test_runner
from mobly.config_parser import TestRunConfig
from mobly.records import TestResultRecord
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
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

REPORTING_SPEED_UNITS = "Mbps"
DAD_TIMEOUT_SEC = 30


@unique
class TrafficDirection(StrEnum):
    RX = auto()
    TX = auto()


@unique
class IPVersion(StrEnum):
    V4 = "ipv4"
    V6 = "ipv6"


@dataclass(frozen=True)
class RateByRange:
    relative_attn: int
    throughput: float


@dataclass(frozen=True)
class TestParams:
    band: Band
    security: Security
    password: str | None
    ip_version: IPVersion
    direction: TrafficDirection


def write_csv_rvr_data(
    test_name: str, csv_path: str, results: list[RateByRange]
) -> None:
    """Writes the CSV data for the RvR test
    Args:
        test_name: The name of test that was run.
        csv_path: Where to put the csv file.
        csv_data: A dictionary of the data to be put in the csv file.
    """
    csv_file_name = f"{csv_path}rvr_throughput_vs_attn_{test_name}.csv"
    with open(csv_file_name, "w+") as csv_fileId:
        csv_fileId.write(
            f"Attenuation(db),Throughput({REPORTING_SPEED_UNITS})\n"
        )
        for res in results:
            csv_fileId.write(f"{res.relative_attn},{res.throughput}\n")


class WlanRvrTest(base_test.WifiBaseTest):
    """Tests running WLAN RvR.

    Test Bed Requirement:
    * One Android device or Fuchsia device
    * One Access Point
    * One attenuator
    * One Linux iPerf Server
    """

    def __init__(self, configs: TestRunConfig) -> None:
        super().__init__(configs)
        self.log = logging.getLogger()
        self.rvr_graph_summary: list[object] = []

        params = MapValidator(self.user_params["rvr_settings"])
        self.starting_attn = params.get(int, "starting_attn", 0)
        self.ending_attn = params.get(int, "ending_attn", 95)
        self.step_size_in_db = params.get(int, "step_size_in_db", 1)
        self.dwell_time_in_secs = params.get(int, "dwell_time_in_secs", 10)

        self.reverse_rvr_after_forward = params.get(
            bool, "reverse_rvr_after_forward", False
        )
        self.iperf_flags = params.get(str, "iperf_flags", "-i 1")
        self.iperf_flags += f" -t {self.dwell_time_in_secs} -J"

        self.fuchsia_device, self.dut = self.get_dut_type(FuchsiaDevice)

        if self.openwrt_aps:
            self.openwrt_ap = self.openwrt_aps[0]
            self.iperf_server = self.openwrt_ap.iperf_server
        elif self.access_points:
            self.access_point = self.access_points[0]
            if len(self.iperf_servers) == 0:
                raise signals.TestAbortClass(
                    "Requires at least one iperf server"
                )
            self.iperf_server = self.iperf_servers[0]
        else:
            raise signals.TestAbortClass("Requires at least one access point")

        ap_controller_type = "OpenWrtAP" if self.openwrt_aps else "AccessPoint"
        self.attenuators_2g = get_attenuators_for_device(
            self.controller_configs[ap_controller_type][0].get(
                "Attenuator", []
            ),
            self.attenuators,
            "attenuator_ports_wifi_2g",
        )
        self.attenuators_5g = get_attenuators_for_device(
            self.controller_configs[ap_controller_type][0].get(
                "Attenuator", []
            ),
            self.attenuators,
            "attenuator_ports_wifi_5g",
        )

        if self.iperf_clients:
            self.dut_iperf_client = self.iperf_clients[0]
        else:
            self.dut_iperf_client = self.dut.create_iperf_client()

    def pre_run(self) -> None:
        test_params: list[TestParams] = []
        securities: list[Security] = [SecurityOpen(), SecurityWpa2()]

        for (
            band,
            security,
            ip_version,
            direction,
        ) in itertools.product(
            [e for e in Band],
            securities,
            [e for e in IPVersion],
            [e for e in TrafficDirection],
        ):
            password: str | None = None
            if not isinstance(security, SecurityOpen):
                password = AccessPointConfig.random_string(20)
            test_params.append(
                TestParams(
                    band,
                    security,
                    password,
                    ip_version,
                    direction,
                )
            )

        def generate_test_name(t: TestParams) -> str:
            # TODO(http://b/303659781): Keep mode in sync with hostapd.
            mode = "11n" if t.band is Band.BAND_2G else "11ac"
            frequency = "20mhz" if t.band is Band.BAND_2G else "80mhz"
            # Map OpenWrt security to hostapd security string to match legacy test name format
            security = ConfigMapper.to_hostapd_security(t.security)
            return (
                f"test_rvr_{mode}_{t.band.lower()}_{frequency}_{security}_"
                f"{t.direction}_{t.ip_version}"
            )

        self.generate_tests(
            self._test_rvr, generate_test_name, [(p,) for p in test_params]
        )

    def setup_test(self) -> None:
        super().setup_test()
        self.iperf_server.start()
        self.dut.disconnect()
        if self.access_point:
            self.access_point.stop_all_aps()

    def teardown_test(self) -> None:
        self.cleanup_tests()
        super().teardown_test()

    def on_fail(self, record: TestResultRecord) -> None:
        super().on_fail(record)
        self.cleanup_tests()

    def cleanup_tests(self) -> None:
        """Cleans up all the dangling pieces of the tests, for example, the
        iperf server, radvd, all the currently running APs, and the various
        clients running during the tests.
        """
        self.download_logs()
        self.iperf_server.stop()
        self.dut.disconnect()
        if self.access_point:
            self.access_point.stop_all_aps()

    def _wait_for_iperf_ipv4_addr(self) -> str:
        """Wait for an IPv4 addresses to become available on the iperf server.

        Returns:
           The private IPv4 address of the iperf server.

        Raises:
            TestFailure: If unable to acquire a IPv4 address.
        """
        ip_address_checker_counter = 0
        ip_address_checker_max_attempts = 3
        while ip_address_checker_counter < ip_address_checker_max_attempts:
            self.iperf_server.renew_test_interface_ip_address()
            iperf_server_ip_addresses = (
                self.iperf_server.get_interface_ip_addresses(
                    self.iperf_server.test_interface
                )
            )
            self.log.info(f"IPerf server IP info: {iperf_server_ip_addresses}")

            if not iperf_server_ip_addresses["ipv4_private"]:
                self.log.warning(
                    "Unable to get the iperf server IPv4 "
                    "address. Retrying..."
                )
                ip_address_checker_counter += 1
                time.sleep(1)
                continue

            return iperf_server_ip_addresses["ipv4_private"][0]

        raise signals.TestFailure("IPv4 address not available on iperf server.")

    def _wait_for_iperf_dad(self) -> str:
        """Wait for Duplicate Address Detection to resolve so that an
        private-local IPv6 address is available for test.

        Returns:
            A string containing the private-local IPv6 address of the iperf server.

        Raises:
            TestFailure: If unable to acquire an IPv6 address.
        """
        now = time.time()
        start = now
        elapsed = now - start

        while elapsed < DAD_TIMEOUT_SEC:
            addrs = self.iperf_server.get_interface_ip_addresses(
                self.iperf_server.test_interface
            )
            now = time.time()
            elapsed = now - start
            if addrs["ipv6_private_local"]:
                # DAD has completed
                addr = addrs["ipv6_private_local"][0]
                self.log.info(
                    f'DAD on iperf server resolved with "{addr}" after {elapsed}s'
                )
                return addr
            time.sleep(1)

        raise signals.TestFailure(
            "Iperf server unable to acquire a private-local IPv6 address for testing "
            f"after {elapsed}s"
        )

    def run_rvr(
        self,
        ssid: str,
        security: Security,
        password: str | None,
        band: Band,
        traffic_dir: TrafficDirection,
        ip_version: IPVersion,
    ) -> list[RateByRange]:
        """Setups and runs the RvR test

        Args:
            ssid: The SSID for the client to associate to.
            security: Security of the AP
            password: Password of the AP
            band: 2g or 5g
            traffic_dir: rx or tx, bi is not supported by iperf3
            ip_version: 4 or 6

        Returns:
            The bokeh graph data.
        """
        match band:
            case Band.BAND_2G:
                rvr_attenuators = self.attenuators_2g
            case Band.BAND_5G:
                rvr_attenuators = self.attenuators_5g

        for rvr_attenuator in rvr_attenuators:
            rvr_attenuator.set_atten(self.starting_attn)

        # Attempt association to the AP multiple times. This makes the test more
        # resilient to AP flakes that may result in the DUT not being able to
        # find the network in its scan results.
        associate_counter = 0
        associate_max_attempts = 3
        while associate_counter < associate_max_attempts:
            self.dut.disconnect()

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
                self.access_point.stop_all_aps()
                legacy_security = HostapdSecurity(
                    security_mode=ConfigMapper.to_hostapd_security(security),
                    password=password,
                )
                self.access_point.start_ap(
                    hostapd_config=create_ap_preset(
                        iface_wlan_2g=self.access_point.wlan_2g,
                        iface_wlan_5g=self.access_point.wlan_5g,
                        profile_name="whirlwind",
                        channel=ConfigMapper.to_hostapd_band(
                            band
                        ).default_channel(),
                        ssid=ssid,
                        security=legacy_security,
                    ),
                    radvd_config=(
                        RadvdConfig() if ip_version is IPVersion.V6 else None
                    ),
                    setup_bridge=True,
                )

            if self.dut.associate(
                ssid,
                target_pwd=password,
                target_security=ConfigMapper.to_hostapd_security(security),
                check_connectivity=False,
            ):
                break
            else:
                associate_counter += 1
        else:
            asserts.fail(
                f"Unable to associate at starting attenuation: {self.starting_attn}"
            )

        match ip_version:
            case IPVersion.V4:
                iperf_server_ip_address = self._wait_for_iperf_ipv4_addr()
            case IPVersion.V6:
                self.iperf_server.renew_test_interface_ip_address()
                self.log.info(
                    "Waiting for iperf server to complete Duplicate "
                    "Address Detection..."
                )
                iperf_server_ip_address = self._wait_for_iperf_dad()

        results = self.rvr_loop(
            traffic_dir,
            rvr_attenuators,
            iperf_server_ip_address,
            ip_version,
            ssid,
            security=security,
            password=password,
            reverse=False,
        )
        if self.reverse_rvr_after_forward:
            results = results + self.rvr_loop(
                traffic_dir,
                rvr_attenuators,
                iperf_server_ip_address,
                ip_version,
                ssid=ssid,
                security=security,
                password=password,
                reverse=True,
            )

        return results

    def rvr_loop(
        self,
        traffic_dir: TrafficDirection,
        rvr_attenuators: list[Attenuator],
        iperf_server_ip_address: str,
        ip_version: IPVersion,
        ssid: str,
        security: Security,
        password: str | None,
        reverse: bool,
    ) -> list[RateByRange]:
        """The loop that goes through each attenuation level and runs the iperf
        throughput pair.
        Args:
            traffic_dir: The traffic direction from the perspective of the DUT.
            rvr_attenuators: A list of attenuators to set.
            iperf_server_ip_address: The IP address of the iperf server.
            ssid: The ssid of the wireless network that the should associated
                to.
            password: Password of the wireless network.
            reverse: Whether to run RvR test starting from the highest
                attenuation and going to the lowest.  This is run after the
                normal low attenuation to high attenuation RvR test.
            throughput: The list of throughput data for the test.
            relative_attn: The list of attenuation data for the test.

        Returns:
            throughput: The list of throughput data for the test.
            relative_attn: The list of attenuation data for the test.
        """
        starting_attn = self.starting_attn
        ending_attn = self.ending_attn
        step_size_in_db = self.step_size_in_db
        if reverse:
            starting_attn = self.ending_attn
            ending_attn = self.starting_attn
            step_size_in_db = step_size_in_db * -1
            self.dut.disconnect()

        results: list[RateByRange] = []

        for step in range(starting_attn, ending_attn, step_size_in_db):
            try:
                for attenuator in rvr_attenuators:
                    self.log.info(
                        f"Setting relative attenuation of {attenuator.instrument.address} "
                        f"to {step} dB"
                    )
                    attenuator.set_atten(step)
            except ValueError as e:
                self.log.error(
                    f"{step} is beyond the max or min of the testbed "
                    f"attenuator's capability. Stopping. {e}"
                )
                break

            self.log.info(f"Running iperf at relative attenuation of {step} dB")

            throughput = self._run_iperf(
                traffic_dir,
                iperf_server_ip_address,
                ip_version,
                ssid,
                security,
                password,
                reverse,
            )
            self.log.info(
                f"Iperf traffic complete. {traffic_dir} traffic received at "
                f"{throughput} {REPORTING_SPEED_UNITS} at relative attenuation "
                f"of {step} db"
            )
            results.append(RateByRange(step, throughput))

        return results

    def _run_iperf(
        self,
        traffic_dir: TrafficDirection,
        iperf_server_ip_address: str,
        ip_version: IPVersion,
        ssid: str,
        security: Security,
        password: str | None,
        reverse: bool,
    ) -> float:
        iperf_flags = self.iperf_flags
        if traffic_dir is TrafficDirection.RX:
            iperf_flags = f"{self.iperf_flags} -R"

        if not self.dut.is_connected():
            if reverse:
                # In reverse mode, we're going from a high attenuation (weak
                # signal) to a low attenuation (strong signal).  It's expected
                # that the DUT is not connected to the AP at the high
                # attenuation level(s), so if we're disconnected here, we
                # should try to associate.
                self.log.info(f"Trying to associate")
                if self.dut.associate(
                    ssid,
                    target_pwd=password,
                    target_security=ConfigMapper.to_hostapd_security(security),
                    check_connectivity=False,
                ):
                    self.log.info("Successfully associated.")
                    try:
                        self.log.debug("Getting DUT IP address")
                        assert self.dut_iperf_client.test_interface is not None
                        if ip_version is IPVersion.V4:
                            self.fuchsia_device.wait_for_ipv4_addr(
                                self.dut_iperf_client.test_interface
                            )
                        elif ip_version is IPVersion.V6:
                            self.fuchsia_device.wait_for_ipv6_addr(
                                self.dut_iperf_client.test_interface
                            )
                    except ConnectionError:
                        self.log.info(
                            f"Association succeeded, but unable to get DUT IP address. Marking a 0 {REPORTING_SPEED_UNITS} "
                            "for throughput. Skipping running traffic and disconnecting."
                        )
                        # Disconnect the DUT, so that we have a fresh attempt
                        # to get an IP at the next iteration of this reverse
                        # test.
                        self.dut.disconnect()
                        return 0
                else:
                    self.log.info(
                        f"Association failed. Marking a 0 {REPORTING_SPEED_UNITS} "
                        "for throughput. Skipping running traffic."
                    )
                    return 0
            else:
                self.log.info(
                    f"Device no longer associated. Marking a 0 {REPORTING_SPEED_UNITS} "
                    "for throughput. Skipping running traffic."
                )
                return 0

        self.log.debug("Pinging iperf server from DUT")
        ping_result = self.dut.ping(iperf_server_ip_address)
        if not ping_result.success:
            self.log.info(
                f'Iperf server "{iperf_server_ip_address}" is not pingable. '
                f"Marking a 0 {REPORTING_SPEED_UNITS} for throughput. "
                "Skipping running traffic."
            )
            self.log.debug(f"{iperf_server_ip_address} pingable: {ping_result}")
            return 0

        self.log.info(f'Iperf server "{iperf_server_ip_address}" is pingable.')

        match traffic_dir:
            case TrafficDirection.TX:
                self.log.info(
                    f"Running traffic from DUT to iperf server ({iperf_server_ip_address})"
                )
            case TrafficDirection.RX:
                self.log.info(
                    f"Running traffic from iperf server ({iperf_server_ip_address}) to DUT"
                )

        try:
            iperf_tag = "decreasing"
            if reverse:
                iperf_tag = "increasing"
            iperf_results_file = self.dut_iperf_client.start(
                iperf_server_ip_address,
                iperf_flags,
                f"{iperf_tag}_{traffic_dir}_{self.starting_attn}",
                timeout=(self.dwell_time_in_secs * 2),
            )
        except TimeoutError as e:
            iperf_results_file = None
            self.log.error(
                f"Iperf traffic timed out. Marking 0 {REPORTING_SPEED_UNITS} for "
                f"throughput. {e}"
            )
            return 0

        if not iperf_results_file:
            return 0

        try:
            iperf_results = IPerfResult(
                iperf_results_file,
                reporting_speed_units=REPORTING_SPEED_UNITS,
            )
            if iperf_results.error:
                self.iperf_server.stop()
                self.iperf_server.start()
                self.log.error(f"Errors in iperf logs:\n{iperf_results.error}")
            if iperf_results.avg_send_rate:
                return iperf_results.avg_send_rate

            self.log.error(
                '"avg_send_rate" not found in iPerf3 results file. Marking 0 '
                f"{REPORTING_SPEED_UNITS} for throughput."
                f"\n{iperf_results.get_json()}"
            )
            return 0
        except ValueError as e:
            self.iperf_server.stop()
            self.iperf_server.start()
            self.log.error(
                f"No data in iPerf3 file. Marking 0 {REPORTING_SPEED_UNITS} "
                f"for throughput: {e}"
            )
            return 0
        except Exception as e:
            self.iperf_server.stop()
            self.iperf_server.start()
            self.log.error(
                f"Unknown exception. Marking 0 {REPORTING_SPEED_UNITS} for "
                f"throughput: {e}"
            )
            return 0

    def _test_rvr(self, t: TestParams) -> None:
        ssid = rand_ascii_str(20)
        if self.access_point:
            self.access_point.stop_all_aps()
        results = self.run_rvr(
            ssid,
            security=t.security,
            password=t.password,
            band=t.band,
            traffic_dir=t.direction,
            ip_version=t.ip_version,
        )
        write_csv_rvr_data(
            self.current_test_info.name,
            self.current_test_info.output_path,
            results,
        )


if __name__ == "__main__":
    test_runner.main()
