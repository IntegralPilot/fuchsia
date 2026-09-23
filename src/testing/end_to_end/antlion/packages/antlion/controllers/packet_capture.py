#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from __future__ import annotations

import io
import logging
import os
import threading
import time
from dataclasses import dataclass

from antlion.controllers.ap_lib.hostapd_constants import (
    CENTER_CHANNEL_MAP,
    FREQUENCY_MAP,
    VHT_CHANNEL,
)
from antlion.libs.proc.process import Process
from antlion.types import ControllerConfig, Json
from libs.ssh import connection, formatter, settings
from mobly import asserts, logger

MOBLY_CONTROLLER_CONFIG_NAME: str = "PacketCapture"
BSS = "BSS"
BSSID = "BSSID"
FREQ = "freq"
FREQUENCY = "frequency"
LEVEL = "level"
MON_2G = "mon0"
MON_5G = "mon1"
BAND_IFACE = {"2G": MON_2G, "5G": MON_5G}
SCAN_IFACE = "wlan2"
SCAN_TIMEOUT = 60
SEP = ":"
SIGNAL = "signal"
SSID = "SSID"


def create(configs: list[ControllerConfig]) -> list[PacketCapture]:
    return [PacketCapture(c) for c in configs]


def destroy(objects: list[PacketCapture]) -> None:
    for pcap in objects:
        pcap.close()


def get_info(objects: list[PacketCapture]) -> list[Json]:
    return [pcap.ssh_settings.hostname for pcap in objects]


@dataclass(frozen=True)
class PcapProperties:
    """Packet capture properties."""

    proc: Process
    """Process object of tcpdump."""

    pcap_fname: str
    """File name of the tcpdump output file."""

    pcap_file: io.BufferedRandom
    """File object for the tcpdump output file."""


class PacketCaptureError(Exception):
    """Error related to Packet capture."""


class PacketCapture(object):
    """Class representing packet capturer.

    An instance of this class creates and configures two interfaces for monitor
    mode; 'mon0' for 2G and 'mon1' for 5G and one interface for scanning for
    wifi networks; 'wlan2' which is a dual band interface.

    Attributes:
        pcap_properties: dict that specifies packet capture properties for a
            band.
    """

    def __init__(self, configs: ControllerConfig) -> None:
        """Initialize objects.

        Args:
            configs: config for the packet capture.
        """
        self.ssh_settings = settings.from_config(configs["ssh_config"])
        self.ssh = connection.SshConnection(self.ssh_settings)
        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[PacketCapture|{self.ssh_settings.hostname}]",
            },
        )

        self._create_interface(MON_2G, "monitor")
        self._create_interface(MON_5G, "monitor")
        self.managed_mode = True
        result = self.ssh.run("ifconfig -a", ignore_status=True)
        if result.stderr or SCAN_IFACE not in result.stdout.decode("utf-8"):
            self.managed_mode = False
        if self.managed_mode:
            self._create_interface(SCAN_IFACE, "managed")

        self.pcap_properties: dict[str, PcapProperties] = {}
        self._pcap_stop_lock = threading.Lock()

    def _create_interface(self, iface: str, mode: str) -> None:
        """Create interface of monitor/managed mode.

        Create mon0/mon1 for 2G/5G monitor mode and wlan2 for managed mode.
        """
        if mode == "monitor":
            self.ssh.run(f"ifconfig wlan{iface[-1]} down", ignore_status=True)
        self.ssh.run(f"iw dev {iface} del", ignore_status=True)
        self.ssh.run(
            f"iw phy{iface[-1]} interface add {iface} type {mode}",
            ignore_status=True,
        )
        self.ssh.run(f"ip link set {iface} up", ignore_status=True)
        result = self.ssh.run(f"iw dev {iface} info", ignore_status=True)
        if result.stderr or iface not in result.stdout.decode("utf-8"):
            raise PacketCaptureError(f"Failed to configure interface {iface}")

    def _cleanup_interface(self, iface: str) -> None:
        """Clean up monitor mode interfaces."""
        self.ssh.run(f"iw dev {iface} del", ignore_status=True)
        result = self.ssh.run(f"iw dev {iface} info", ignore_status=True)
        if not result.stderr or "No such device" not in result.stderr.decode(
            "utf-8"
        ):
            raise PacketCaptureError(
                f"Failed to cleanup monitor mode for {iface}"
            )

    def _parse_scan_results(
        self, scan_result: str
    ) -> list[dict[str, str | int]]:
        """Parses the scan dump output and returns list of dictionaries.

        Args:
            scan_result: scan dump output from scan on mon interface.

        Returns:
            Dictionary of found network in the scan.
            The attributes returned are
                a.) SSID - SSID of the network.
                b.) LEVEL - signal level.
                c.) FREQUENCY - WiFi band the network is on.
                d.) BSSID - BSSID of the network.
        """
        scan_networks: list[dict[str, str | int]] = []
        network: dict[str, str | int] = {}
        for line in scan_result.splitlines():
            if SEP not in line:
                continue
            if BSS in line:
                network[BSSID] = line.split("(")[0].split()[-1]
            field, value = line.lstrip().rstrip().split(SEP)[0:2]
            value = value.lstrip()
            if SIGNAL in line:
                network[LEVEL] = int(float(value.split()[0]))
            elif FREQ in line:
                network[FREQUENCY] = int(value)
            elif SSID in line:
                network[SSID] = value
                scan_networks.append(network)
                network = {}
        return scan_networks

    def get_wifi_scan_results(self) -> list[dict[str, str | int]]:
        """Starts a wifi scan on wlan2 interface.

        Returns:
            List of dictionaries each representing a found network.
        """
        if not self.managed_mode:
            raise PacketCaptureError("Managed mode not setup")
        result = self.ssh.run(f"iw dev {SCAN_IFACE} scan")
        if result.stderr:
            raise PacketCaptureError("Failed to get scan dump")
        if not result.stdout:
            return []
        return self._parse_scan_results(result.stdout.decode("utf-8"))

    def start_scan_and_find_network(self, ssid: str) -> bool:
        """Start a wifi scan on wlan2 interface and find network.

        Args:
            ssid: SSID of the network.

        Returns:
            True/False if the network if found or not.
        """
        curr_time = time.time()
        while time.time() < curr_time + SCAN_TIMEOUT:
            found_networks = self.get_wifi_scan_results()
            for network in found_networks:
                if network[SSID] == ssid:
                    return True
            time.sleep(3)  # sleep before next scan
        return False

    def configure_monitor_mode(
        self, band: str, channel: int, bandwidth: int = 20
    ) -> bool:
        """Configure monitor mode.

        Args:
            band: band to configure monitor mode for.
            channel: channel to set for the interface.
            bandwidth : bandwidth for VHT channel as 40,80,160

        Returns:
            True if configure successful.
            False if not successful.
        """

        band = band.upper()
        if band not in BAND_IFACE:
            self.log.error("Invalid band. Must be 2g/2G or 5g/5G")
            return False

        iface = BAND_IFACE[band]
        if bandwidth == 20:
            self.ssh.run(
                f"iw dev {iface} set channel {channel}", ignore_status=True
            )
        else:
            center_freq = None
            for i, j in CENTER_CHANNEL_MAP[VHT_CHANNEL[bandwidth]]["channels"]:
                if channel in range(i, j + 1):
                    center_freq = (FREQUENCY_MAP[i] + FREQUENCY_MAP[j]) / 2
                    break
            asserts.assert_true(
                center_freq, "No match channel in VHT channel list."
            )
            self.ssh.run(
                "iw dev %s set freq %s %s %s"
                % (iface, FREQUENCY_MAP[channel], bandwidth, center_freq),
                ignore_status=True,
            )

        result = self.ssh.run(f"iw dev {iface} info", ignore_status=True)
        if result.stderr or f"channel {channel}" not in result.stdout.decode(
            "utf-8"
        ):
            self.log.error(f"Failed to configure monitor mode for {band}")
            return False
        return True

    def start_packet_capture(
        self, band: str, log_path: str, pcap_fname: str
    ) -> Process | None:
        """Start packet capture for band.

        band = 2G starts tcpdump on 'mon0' interface.
        band = 5G starts tcpdump on 'mon1' interface.

        Args:
            band: '2g' or '2G' and '5g' or '5G'.
            log_path: test log path to save the pcap file.
            pcap_fname: name of the pcap file.

        Returns:
            pcap_proc: Process object of the tcpdump.
        """
        band = band.upper()
        if band not in BAND_IFACE.keys() or band in self.pcap_properties:
            self.log.error("Invalid band or packet capture already running")
            return None

        pcap_name = f"{pcap_fname}_{band}.pcap"
        pcap_fname = os.path.join(log_path, pcap_name)
        pcap_file = open(pcap_fname, "w+b")

        tcpdump_cmd = f"tcpdump -i {BAND_IFACE[band]} -w - -U 2>/dev/null"
        cmd = formatter.SshFormatter().format_command(
            tcpdump_cmd, self.ssh_settings, extra_flags={"-q": None}
        )
        pcap_proc = Process(cmd)

        def write_to_pcap(data: bytes | str) -> None:
            if isinstance(data, str):
                data = data.encode("utf-8")
            pcap_file.write(data)

        pcap_proc.set_on_output_callback(write_to_pcap, binary=True)
        pcap_proc.start()

        self.pcap_properties[band] = PcapProperties(
            pcap_proc, pcap_fname, pcap_file
        )
        return pcap_proc

    def stop_packet_capture(self, proc: Process) -> None:
        """Stop the packet capture.

        Args:
            proc: Process object of tcpdump to kill.
        """
        for key, val in self.pcap_properties.items():
            if val.proc is proc:
                break
        else:
            self.log.error("Failed to stop tcpdump. Invalid process.")
            return

        proc.stop()
        with self._pcap_stop_lock:
            self.pcap_properties[key].pcap_file.close()
            del self.pcap_properties[key]

    def close(self) -> None:
        """Cleanup.

        Cleans up all the monitor mode interfaces and closes ssh connections.
        """
        self._cleanup_interface(MON_2G)
        self._cleanup_interface(MON_5G)
        self.ssh.close()
