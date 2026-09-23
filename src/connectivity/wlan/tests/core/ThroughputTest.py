# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import csv
import html
import json
import logging
import os
from dataclasses import dataclass
from datetime import timedelta
from typing import Literal, TypedDict, overload

import fidl_fuchsia_wlan_internal as fidl_security
import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from antlion.controllers import iperf_server
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib.hostapd import (
    StationStatus as HostapdStationStatus,
)
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.iperf_server import IPerfServerOverSsh
from honeydew.affordances.connectivity.netstack.types import PortClass
from honeydew.affordances.connectivity.wlan.utils.types import (
    KNOWN_COUNTRY_CODES,
)
from honeydew.typing.custom_types import MacAddress
from mobly import asserts, signals, test_runner
from mobly.config_parser import TestRunConfig
from openwrt_access_point import StationStatus as OpenWrtStationStatus
from openwrt_access_point.lib.access_point_config import (
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    HeMode,
    RadioConfig,
    Security,
    SecurityWpa2,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)

logger = logging.getLogger(__name__)

IPERF_DURATION: timedelta = timedelta(seconds=10)


@dataclass
class LinkMetrics:
    """Represents link quality and rate metrics perceived by AP and DUT."""

    ap_rssi: int | None = None
    ap_snr: int | None = None
    ap_tx_rate_mbps: float | None = None
    ap_rx_rate_mbps: float | None = None
    ap_phy_mode: str | None = None
    ap_nss: int | None = None
    dut_rssi: int | None = None
    dut_tx_rate_mbps: float | None = None
    dut_rx_rate_mbps: float | None = None


@dataclass
class MetricStats:
    avg: float | None = None
    min: float | int | None = None
    max: float | int | None = None

    @classmethod
    def from_values(
        cls, values: list[int | float | None], round_digits: int = 1
    ) -> "MetricStats":
        valid = [v for v in values if v is not None]
        if not valid:
            return cls()
        return cls(
            avg=round(sum(valid) / len(valid), round_digits),
            min=min(valid),
            max=max(valid),
        )


@dataclass
class LinkMetricSummary:
    """Aggregated link quality and rate statistics across all measurement samples."""

    ap_rssi: MetricStats
    ap_snr: MetricStats
    ap_tx_rate_mbps: MetricStats
    ap_rx_rate_mbps: MetricStats
    dut_rssi: MetricStats
    dut_tx_rate_mbps: MetricStats
    dut_rx_rate_mbps: MetricStats
    phy_mode: str | None
    nss: int | None
    samples: list[LinkMetrics]

    @classmethod
    def from_samples(cls, samples: list[LinkMetrics]) -> "LinkMetricSummary":
        # Find the most recent valid phy_mode and nss from the collected samples,
        # reflecting the steady-state negotiated configuration under load.
        phy_mode: str | None = None
        for s in reversed(samples):
            if s.ap_phy_mode:
                phy_mode = s.ap_phy_mode
                break

        nss: int | None = None
        for s in reversed(samples):
            if s.ap_nss is not None:
                nss = s.ap_nss
                break

        return cls(
            ap_rssi=MetricStats.from_values(
                [s.ap_rssi for s in samples], round_digits=1
            ),
            ap_snr=MetricStats.from_values(
                [s.ap_snr for s in samples], round_digits=1
            ),
            ap_tx_rate_mbps=MetricStats.from_values(
                [s.ap_tx_rate_mbps for s in samples], round_digits=1
            ),
            ap_rx_rate_mbps=MetricStats.from_values(
                [s.ap_rx_rate_mbps for s in samples], round_digits=1
            ),
            dut_rssi=MetricStats.from_values(
                [s.dut_rssi for s in samples], round_digits=1
            ),
            dut_tx_rate_mbps=MetricStats.from_values(
                [s.dut_tx_rate_mbps for s in samples], round_digits=1
            ),
            dut_rx_rate_mbps=MetricStats.from_values(
                [s.dut_rx_rate_mbps for s in samples], round_digits=1
            ),
            phy_mode=phy_mode,
            nss=nss,
            samples=samples,
        )

    def format_summary(self) -> str:
        def fmt_rate(val: float | None) -> str:
            return f"{val:.1f} Mbps" if val is not None else "N/A"

        def fmt_rssi(val: float | None) -> str:
            return f"{val:.1f} dBm" if val is not None else "N/A"

        def fmt_snr(val: float | None) -> str:
            return f"{val:.1f} dB" if val is not None else "N/A"

        mode_str = self.phy_mode or "N/A"
        nss_str = str(self.nss) if self.nss is not None else "N/A"

        return (
            f"AP PHY (Mode: {mode_str}, NSS: {nss_str}, "
            f"RSSI: {fmt_rssi(self.ap_rssi.avg)}, "
            f"SNR: {fmt_snr(self.ap_snr.avg)}, "
            f"TX: {fmt_rate(self.ap_tx_rate_mbps.avg)}, "
            f"RX: {fmt_rate(self.ap_rx_rate_mbps.avg)}), "
            f"DUT PHY (RSSI: {fmt_rssi(self.dut_rssi.avg)}, "
            f"TX: {fmt_rate(self.dut_tx_rate_mbps.avg)}, "
            f"RX: {fmt_rate(self.dut_rx_rate_mbps.avg)})"
        )

    def format_sample_table(self) -> list[str]:
        if not self.samples:
            return []

        def r_val(val: float | None) -> str:
            return f"{val:.1f}" if val is not None else "N/A"

        def v_val(val: int | None) -> str:
            return str(val) if val is not None else "N/A"

        rows = [
            (
                "  RSSI (ap/dut):",
                [
                    f"{v_val(s.ap_rssi)}/{v_val(s.dut_rssi)}"
                    for s in self.samples
                ],
            ),
            (
                "  SNR (ap):",
                [f"{v_val(s.ap_snr)}" for s in self.samples],
            ),
            (
                "  PHY (ap_tx/dut_rx):",
                [
                    f"{r_val(s.ap_tx_rate_mbps)}/{r_val(s.dut_rx_rate_mbps)}"
                    for s in self.samples
                ],
            ),
            (
                "  PHY (ap_rx/dut_tx):",
                [
                    f"{r_val(s.ap_rx_rate_mbps)}/{r_val(s.dut_tx_rate_mbps)}"
                    for s in self.samples
                ],
            ),
        ]
        col_width = max(len(c) for _, cells in rows for c in cells)
        prefix_width = max(len(prefix) for prefix, _ in rows) + 1
        return [
            f"{prefix.ljust(prefix_width)}{'  '.join(c.ljust(col_width) for c in cells)}"
            for prefix, cells in rows
        ]

    def to_csv_strings(self) -> list[str]:
        def fmt_avg(val: int | float | None) -> str:
            if val is None:
                return ""
            return f"{val:.1f}"

        def fmt_val(val: int | float | None) -> str:
            if val is None:
                return ""
            if isinstance(val, float) and val.is_integer():
                return str(int(val))
            return str(val)

        def fmt_rate(val: int | float | None) -> str:
            if val is None:
                return ""
            return f"{val:.1f}"

        vals: list[str] = []
        for s in [self.ap_rssi, self.ap_snr]:
            vals.extend([fmt_avg(s.avg), fmt_val(s.min), fmt_val(s.max)])
        for s in [self.ap_tx_rate_mbps, self.ap_rx_rate_mbps]:
            vals.extend([fmt_rate(s.avg), fmt_rate(s.min), fmt_rate(s.max)])
        for s in [self.dut_rssi]:
            vals.extend([fmt_avg(s.avg), fmt_val(s.min), fmt_val(s.max)])
        for s in [self.dut_tx_rate_mbps, self.dut_rx_rate_mbps]:
            vals.extend([fmt_rate(s.avg), fmt_rate(s.min), fmt_rate(s.max)])
        return vals


class IperfUdpResult(TypedDict):
    udp_bps_uncorrected: int
    udp_bps_corrected: int
    udp_loss_percent: float
    server_cpu_utilization_percent: float


class IperfTcpResult(TypedDict):
    tcp_bps: int
    server_cpu_utilization_percent: float


class IperfResult(TypedDict):
    tcp_tx: IperfTcpResult
    tcp_rx: IperfTcpResult
    udp_tx: IperfUdpResult
    udp_rx: IperfUdpResult


@dataclass
class TestParams:
    security_mode: Security
    band: Band
    channel: int
    channel_bandwidth: Literal[20, 40, 80, 160]


class ThroughputTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy
    iperf_server: IPerfServerOverSsh

    def __init__(self, configs: TestRunConfig) -> None:
        super().__init__(configs)
        self.csv_file_path: str = os.path.join(self.log_path, "throughput.csv")
        self.html_file_path: str = os.path.join(
            self.log_path, "throughput.html"
        )

    async def pre_run(self) -> None:
        tests: list[tuple[TestParams]] = []

        def generate_test_name(test: TestParams) -> str:
            sec_name = ConfigMapper.to_hostapd_security(
                test.security_mode
            ).value
            return f"test_{sec_name}_channel_{test.channel}_{test.channel_bandwidth}mhz"

        channels_and_bandwidths: list[
            tuple[Band, int, Literal[20, 40, 80, 160]]
        ] = [
            # 20MHz is the only realistic BW in 2.4GHz
            (Band.BAND_2G, 1, 20),
            # Highest US channel, 20MHz to compare to 2.4GHz
            (Band.BAND_5G, 165, 20),
            # Widest possible bandwidth in 5GHz supported by Whirlwind APs.
            # Once we move to OpenWRT One, we can raise this to 160MHz.
            (Band.BAND_5G, 36, 80),
        ]
        security_modes: list[Security] = [
            SecurityWpa2(),
        ]
        for security_mode in security_modes:
            for band, channel, bandwidth in channels_and_bandwidths:
                test = TestParams(
                    security_mode=security_mode,
                    band=band,
                    channel=channel,
                    channel_bandwidth=bandwidth,
                )
                tests.append((test,))

        self.generate_tests(
            self.run_channel_performance, generate_test_name, tests
        )

    async def setup_class(self) -> None:
        await super().setup_class()
        self.phy = await self.dut.wlan_core.ensure_single_phy()
        await self.phy.set_country(
            KNOWN_COUNTRY_CODES["UNITED_STATES_OF_AMERICA"]
        )

        iperf_servers: list[IPerfServerOverSsh] = (
            await self.register_controller(
                iperf_server,
                required=False,
            )
            or []
        )

        if self.openwrt_ap is not None:
            self.iperf_server = self.openwrt_ap.iperf_server
        elif iperf_servers:
            self.iperf_server = iperf_servers[0]
        else:
            raise signals.TestError("Requires at least one iperf server")
        self.iperf_server.start()

        with open(self.csv_file_path, "w", encoding="utf-8") as csv_file:
            csv_file.write(
                "security,channel,bandwidth,phy_mode,nss,"
                + "udp_or_tcp,dut_tx_or_rx,result_mbps,"
                + "ap_rssi_avg,ap_rssi_min,ap_rssi_max,"
                + "ap_snr_avg,ap_snr_min,ap_snr_max,"
                + "ap_tx_rate_mbps_avg,ap_tx_rate_mbps_min,ap_tx_rate_mbps_max,"
                + "ap_rx_rate_mbps_avg,ap_rx_rate_mbps_min,ap_rx_rate_mbps_max,"
                + "dut_rssi_avg,dut_rssi_min,dut_rssi_max,"
                + "dut_tx_rate_mbps_avg,dut_tx_rate_mbps_min,dut_tx_rate_mbps_max,"
                + "dut_rx_rate_mbps_avg,dut_rx_rate_mbps_min,dut_rx_rate_mbps_max\n"
            )

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_core.destroy_all_ifaces()

    async def teardown_test(self) -> None:
        if self.access_point is not None:
            self.access_point.stop_all_aps()
        await super().teardown_test()

    async def teardown_class(self) -> None:
        self._update_html_report()
        await super().teardown_class()

    async def _measure_link_metrics(
        self, iface: wlan_core.ClientIface, dut_mac: MacAddress, band: Band
    ) -> LinkMetrics:
        metrics = LinkMetrics()

        # 1. Query AP perspective
        ap_status: OpenWrtStationStatus | HostapdStationStatus
        if self.openwrt_ap is not None:
            ap_status = await asyncio.to_thread(
                self.openwrt_ap.get_sta_status, dut_mac, band
            )
        elif isinstance(self.access_point, AccessPoint):
            ap_iface = (
                self.access_point.wlan_2g
                if band == Band.BAND_2G
                else self.access_point.wlan_5g
            )
            ap_status = await asyncio.to_thread(
                self.access_point.get_sta_status, ap_iface, dut_mac
            )
        else:
            raise RuntimeError(
                "No AP controller available to query station status"
            )
        if ap_status.rssi is None:
            raise signals.TestFailure(
                f"Expected AP RSSI to be present for {dut_mac}: {ap_status}"
            )
        if ap_status.tx_rate_mbps is None:
            raise signals.TestFailure(
                f"Expected AP TX PHY rate to be present for {dut_mac}: {ap_status}"
            )
        asserts.assert_greater(
            ap_status.tx_rate_mbps,
            0,
            f"Expected positive AP TX PHY rate for {dut_mac}: {ap_status}",
        )
        if ap_status.rx_rate_mbps is None:
            raise signals.TestFailure(
                f"Expected AP RX PHY rate to be present for {dut_mac}: {ap_status}"
            )
        asserts.assert_greater(
            ap_status.rx_rate_mbps,
            0,
            f"Expected positive AP RX PHY rate for {dut_mac}: {ap_status}",
        )
        metrics.ap_rssi = ap_status.rssi
        metrics.ap_snr = ap_status.snr
        metrics.ap_tx_rate_mbps = ap_status.tx_rate_mbps
        metrics.ap_rx_rate_mbps = ap_status.rx_rate_mbps
        metrics.ap_phy_mode = ap_status.phy_mode
        metrics.ap_nss = ap_status.nss

        # 2. Query DUT perspective
        signal_report = await iface.get_signal_report()
        conn_report = getattr(signal_report, "connection_signal_report", None)
        if conn_report is None:
            raise signals.TestFailure(
                f"DUT signal report missing connection_signal_report: {signal_report}"
            )
        dut_rssi = getattr(conn_report, "rssi_dbm", None)
        if dut_rssi is None:
            raise signals.TestFailure(
                f"DUT connection signal report missing rssi_dbm: {conn_report}"
            )
        metrics.dut_rssi = dut_rssi

        tx_rate_500kbps = getattr(conn_report, "tx_rate_500kbps", None)
        if tx_rate_500kbps is None:
            raise signals.TestFailure(
                f"DUT connection signal report missing tx_rate_500kbps: {conn_report}"
            )
        asserts.assert_greater(
            tx_rate_500kbps,
            0,
            f"DUT connection signal report non-positive tx_rate_500kbps: {tx_rate_500kbps}",
        )
        metrics.dut_tx_rate_mbps = round(tx_rate_500kbps * 0.5, 2)
        # Note: ConnectionSignalReport does not include an RX rate, so
        # metrics.dut_rx_rate_mbps remains None.

        return metrics

    @overload
    async def get_iperf_throughput_bps(
        self,
        iperf_server_address: str,
        reverse: bool,
        udp: Literal[False],
        iface: wlan_core.ClientIface,
        dut_mac: MacAddress,
        band: Band,
        bandwidth: None = None,
    ) -> tuple[IperfTcpResult, LinkMetricSummary]:
        ...

    @overload
    async def get_iperf_throughput_bps(
        self,
        iperf_server_address: str,
        reverse: bool,
        udp: Literal[True],
        iface: wlan_core.ClientIface,
        dut_mac: MacAddress,
        band: Band,
        bandwidth: str | None = None,
    ) -> tuple[IperfUdpResult, LinkMetricSummary]:
        ...

    async def get_iperf_throughput_bps(
        self,
        iperf_server_address: str,
        reverse: bool,
        udp: bool,
        iface: wlan_core.ClientIface,
        dut_mac: MacAddress,
        band: Band,
        bandwidth: str | None = None,
    ) -> tuple[IperfUdpResult | IperfTcpResult, LinkMetricSummary]:
        args = [
            "--client",
            iperf_server_address,
            "--time",
            str(int(IPERF_DURATION.total_seconds())),
            "--format",
            "m",  # 'm' = Mbits/sec
            "--json",
        ]
        if udp:
            args.extend(["--udp", "--bandwidth", bandwidth or "0"])
        if reverse:
            args.append("--reverse")

        # Launch iperf3 in a background thread so link measurements can be taken mid-test
        cmd = f"iperf3 {' '.join(args)}"
        iperf_task = asyncio.create_task(
            asyncio.to_thread(self.dut.ffx.run_ssh_cmd, cmd=cmd)
        )

        start_time = asyncio.get_running_loop().time()
        samples: list[LinkMetrics] = []
        try:
            loop = asyncio.get_running_loop()
            # Sample link metrics once every second during the test run, starting at
            # t = 1s and ending before the iperf duration elapses.
            for target_second in range(1, int(IPERF_DURATION.total_seconds())):
                # Compensate for measurement drift to maintain 1-second intervals.
                sleep_duration = (start_time + target_second) - loop.time()
                if sleep_duration > 0:
                    done, _ = await asyncio.wait(
                        [iperf_task], timeout=sleep_duration
                    )
                    if done:
                        break
                elif iperf_task.done():
                    break

                samples.append(
                    await self._measure_link_metrics(iface, dut_mac, band)
                )

            # Wait for iperf3 to complete
            output = await iperf_task
        except Exception:
            if not iperf_task.done():
                iperf_task.cancel()
            raise

        # Check for iperf3 errors before verifying samples were collected
        try:
            data = json.loads(output)
            if "error" in data:
                raise signals.TestError(f"iperf3 error: {data['error']}")
        except json.JSONDecodeError as e:
            logger.error("Failed to parse data from command output:")
            logger.error(output)
            raise signals.TestError(f"Invalid JSON from iperf3: {e}") from e

        asserts.assert_true(
            samples, "Expected at least one link metrics sample"
        )
        metrics = LinkMetricSummary.from_samples(samples)

        end_data = data.get("end", {})

        server_cpu_utilization_percent = end_data.get(
            "cpu_utilization_percent", {}
        ).get("remote_total")
        if server_cpu_utilization_percent is None:
            logger.error(
                f"Could not extract cpu_utilization_percent from iperf3 JSON: {data}"
            )
            raise signals.TestError(
                "Could not extract cpu_utilization_percent from iperf3 JSON"
            )

        if udp:
            raw_udp_bps = end_data.get("sum", {}).get("bits_per_second")
            lost_percent = end_data.get("sum", {}).get("lost_percent")
            if raw_udp_bps is None:
                logger.error(
                    f"Could not extract bits_per_second from iperf3 JSON: {data}"
                )
                raise signals.TestError(
                    "Could not extract bits_per_second from iperf3 JSON"
                )
            bps = raw_udp_bps * (1.0 - (lost_percent / 100.0))

            res_udp: IperfUdpResult = {
                "udp_bps_uncorrected": raw_udp_bps,
                "udp_bps_corrected": bps,
                "udp_loss_percent": lost_percent,
                "server_cpu_utilization_percent": server_cpu_utilization_percent,
            }
            return res_udp, metrics
        else:
            bps = end_data.get("sum_received", {}).get("bits_per_second")
            if bps is None:
                logger.error(
                    f"Could not extract bits_per_second from iperf3 JSON: {data}"
                )
                raise signals.TestError(
                    "Could not extract bits_per_second from iperf3 JSON"
                )

            res_tcp: IperfTcpResult = {
                "tcp_bps": bps,
                "server_cpu_utilization_percent": server_cpu_utilization_percent,
            }
            return res_tcp, metrics

    def _log_measurement_result(
        self, test_label: str, mbps: float, metrics: LinkMetricSummary
    ) -> None:
        logger.info(
            f"{test_label} Result: {mbps} Mbps | {metrics.format_summary()}"
        )
        for line in metrics.format_sample_table():
            logger.info(line)

    def _log_udp_correction(
        self, test_label: str, udp_res: IperfUdpResult, tested_send_speed: str
    ) -> None:
        uncorrected_mbps = self.bps_to_mbps(udp_res["udp_bps_uncorrected"])
        corrected_mbps = self.bps_to_mbps(udp_res["udp_bps_corrected"])
        loss_percent = round(udp_res["udp_loss_percent"], 2)
        logger.info(
            f"{test_label} UDP Correction: tested send speed: {tested_send_speed}, "
            f"loss: {loss_percent}%, "
            f"uncorrected: {uncorrected_mbps} Mbps, "
            f"corrected: {corrected_mbps} Mbps"
        )

    async def run_channel_performance(self, test: TestParams) -> None:
        iface = await self.phy.create_client_iface()
        ssid = AccessPointConfig.random_string(8)
        password = AccessPointConfig.random_string(8)
        security = test.security_mode
        band = test.band

        if self.openwrt_ap:
            # HE / Wifi 6 / 802.11ax is the latest supported by OpenWRT One
            phy_mode = HeMode(bw=test.channel_bandwidth)
            self.openwrt_ap.configure_wifi(
                AccessPointConfig(
                    radios=[
                        RadioConfig(
                            channel=BssChannel(
                                band=band,
                                number=test.channel,
                                phy_mode=phy_mode,
                            ),
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid,
                                    password=password,
                                    security=security,
                                )
                            ],
                        )
                    ]
                )
            )
        elif isinstance(self.access_point, AccessPoint):
            setup_ap(
                access_point=self.access_point,
                profile_name="whirlwind",
                channel=test.channel,
                ssid=ssid,
                vht_bandwidth=test.channel_bandwidth,
                security=DeprecatedSecurity(
                    security_mode=ConfigMapper.to_hostapd_security(security),
                    password=password,
                ),
                force_wmm=True,  # Gets maximum speed in 802.11ac
                setup_bridge=True,  # Necessary for the separate iperf server
            )

        await iface.scan_and_connect(
            ssid=ssid,
            password=password,
            security=fidl_security.Protocol.WPA2_PERSONAL,
        )

        if self.openwrt_ap is None:
            # Necessary while the iperf server is separate from the AP
            self.iperf_server.renew_test_interface_ip_address()
        iperf_server_address = self.iperf_server.get_addr()

        # Wait for IP routing to settle before running throughput tests
        netstack_iface = await self.dut.netstack.wait_for_interface(
            PortClass.WLAN_CLIENT
        )
        dut_mac = await iface.get_mac_address()
        asserts.assert_equal(netstack_iface.mac, dut_mac)
        await self.dut.netstack.wait_for_ipv4_addr(netstack_iface.id_)

        udp_bandwidth = self.get_target_udp_bandwidth(test.channel_bandwidth)
        tcp_tx, tcp_tx_metrics = await self.get_iperf_throughput_bps(
            iperf_server_address,
            reverse=False,
            udp=False,
            iface=iface,
            dut_mac=dut_mac,
            band=band,
        )
        tcp_tx_mbps = self.bps_to_mbps(tcp_tx["tcp_bps"])
        self._log_measurement_result("TCP TX", tcp_tx_mbps, tcp_tx_metrics)

        tcp_rx, tcp_rx_metrics = await self.get_iperf_throughput_bps(
            iperf_server_address,
            reverse=True,
            udp=False,
            iface=iface,
            dut_mac=dut_mac,
            band=band,
        )
        tcp_rx_mbps = self.bps_to_mbps(tcp_rx["tcp_bps"])
        self._log_measurement_result("TCP RX", tcp_rx_mbps, tcp_rx_metrics)

        udp_tx, udp_tx_metrics = await self.get_iperf_throughput_bps(
            iperf_server_address,
            reverse=False,
            udp=True,
            iface=iface,
            dut_mac=dut_mac,
            band=band,
            bandwidth=udp_bandwidth,
        )
        udp_tx_mbps = self.bps_to_mbps(udp_tx["udp_bps_corrected"])
        self._log_udp_correction("UDP TX", udp_tx, udp_bandwidth)
        self._log_measurement_result("UDP TX", udp_tx_mbps, udp_tx_metrics)

        udp_rx, udp_rx_metrics = await self.get_iperf_throughput_bps(
            iperf_server_address,
            reverse=True,
            udp=True,
            iface=iface,
            dut_mac=dut_mac,
            band=band,
            bandwidth=udp_bandwidth,
        )
        udp_rx_mbps = self.bps_to_mbps(udp_rx["udp_bps_corrected"])
        self._log_udp_correction("UDP RX", udp_rx, udp_bandwidth)
        self._log_measurement_result("UDP RX", udp_rx_mbps, udp_rx_metrics)

        sec_name = ConfigMapper.to_hostapd_security(test.security_mode).value
        with open(self.csv_file_path, "a", encoding="utf-8") as csv_file:
            for proto, direction, metrics, mbps in [
                ("tcp", "tx", tcp_tx_metrics, tcp_tx_mbps),
                ("tcp", "rx", tcp_rx_metrics, tcp_rx_mbps),
                ("udp", "tx", udp_tx_metrics, udp_tx_mbps),
                ("udp", "rx", udp_rx_metrics, udp_rx_mbps),
            ]:
                row = [
                    sec_name,
                    str(test.channel),
                    str(test.channel_bandwidth),
                    metrics.phy_mode or "",
                    str(metrics.nss) if metrics.nss is not None else "",
                    proto,
                    direction,
                    str(mbps),
                    *metrics.to_csv_strings(),
                ]
                csv_file.write(",".join(row) + "\n")

    @staticmethod
    def get_target_udp_bandwidth(channel_bandwidth: int) -> str:
        """Determines target UDP bitrate scaled to channel width for iperf3.

        Why this is needed:
            In iperf3 UDP tests, setting unlimited bandwidth (`--bandwidth 0`)
            causes the sender (especially the wired iperf server in reverse mode)
            to transmit at line rate (1 Gbps) into the Access Point.
            On narrower channels (e.g. 2.4 GHz 20 MHz where link capacity is only
            ~30-50 Mbps), this massive rate mismatch causes severe bufferbloat and
            >95% packet loss in the AP queues. This drops the TCP control socket
            packets on port 5201, causing iperf3 to abort with "control socket has
            closed unexpectedly".

            Setting the target UDP bandwidth to slightly exceed the theoretical
            maximum physical layer (PHY) capacity ensures the wireless medium is
            fully saturated without overwhelming the AP's packet queues.

        Calculation per channel bandwidth:
            - 20 MHz -> "150M": Max PHY rates are 72.2 Mbps (802.11n 1x1),
              144.4 Mbps (802.11n 2x2), and 143.4 Mbps (802.11ax 1x1).
              150 Mbps ensures saturation while keeping queue drops manageable.
            - 40 MHz -> "300M": Max PHY rates are ~150-300 Mbps (802.11n/ac)
              and 286.8 Mbps (802.11ax 1x1).
            - 80 MHz -> "600M": Max PHY rate for 802.11ac 1x1 is 433.3 Mbps
              (and typical 2x2 real-world throughput is ~400-500 Mbps).
            - 160 MHz -> "1000M": Capped at 1 Gbps to match the wired Ethernet
              link speed between the iperf server and the Access Point.
        """
        if channel_bandwidth <= 20:
            return "150M"
        elif channel_bandwidth <= 40:
            return "300M"
        elif channel_bandwidth <= 80:
            return "600M"
        else:
            return "1000M"

    @staticmethod
    def bps_to_mbps(bps: int | float) -> float:
        throughput_mbps = float(bps) / 1_000_000
        return round(throughput_mbps, 2)

    def _update_html_report(self) -> None:
        """Renders throughput.csv into a clean, human-readable HTML table."""
        if not os.path.exists(self.csv_file_path):
            return

        with open(self.csv_file_path, "r", encoding="utf-8") as f:
            rows = list(csv.reader(f))

        if not rows:
            return

        header_row = rows[0]
        data_rows = rows[1:]

        html_lines: list[str] = [
            "<!DOCTYPE html>",
            '<html lang="en">',
            "<head>",
            '  <meta charset="utf-8">',
            "  <title>Throughput Test Results</title>",
            "  <style>",
            "    body {",
            '      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;',
            "      margin: 24px;",
            "      color: #202124;",
            "      background-color: #ffffff;",
            "    }",
            "    h2 {",
            "      margin-bottom: 16px;",
            "    }",
            "    table {",
            "      border-collapse: collapse;",
            "      font-size: 14px;",
            "      box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);",
            "    }",
            "    th, td {",
            "      border: 1px solid #dadce0;",
            "      padding: 8px 14px;",
            "      white-space: nowrap;",
            "    }",
            "    th {",
            "      background-color: #f1f3f4;",
            "      font-weight: 600;",
            "      text-align: center;",
            "    }",
            "    tr:nth-child(even) {",
            "      background-color: #f8f9fa;",
            "    }",
            "    tr:hover {",
            "      background-color: #f1f3f4;",
            "    }",
            "    .num {",
            "      text-align: right;",
            "      font-variant-numeric: tabular-nums;",
            "    }",
            "    .str {",
            "      text-align: left;",
            "    }",
            "    .na {",
            "      color: #9aa0a6;",
            "      text-align: center;",
            "    }",
            "  </style>",
            "</head>",
            "<body>",
            "  <h2>Throughput Test Results</h2>",
            "  <table>",
            "    <thead>",
            "      <tr>",
        ]

        for h in header_row:
            html_lines.append(f"        <th>{html.escape(h)}</th>")
        html_lines.extend(
            [
                "      </tr>",
                "    </thead>",
                "    <tbody>",
            ]
        )

        for row in data_rows:
            html_lines.append("      <tr>")
            for cell in row:
                cell_str = cell.strip()
                if not cell_str:
                    html_lines.append('        <td class="na">N/A</td>')
                else:
                    try:
                        float(cell_str)
                        cls = "num"
                    except ValueError:
                        cls = "str"
                    html_lines.append(
                        f'        <td class="{cls}">{html.escape(cell_str)}</td>'
                    )
            html_lines.append("      </tr>")

        html_lines.extend(
            [
                "    </tbody>",
                "  </table>",
                "</body>",
                "</html>\n",
            ]
        )

        with open(self.html_file_path, "w", encoding="utf-8") as f:
            f.write("\n".join(html_lines))


if __name__ == "__main__":
    test_runner.main()
