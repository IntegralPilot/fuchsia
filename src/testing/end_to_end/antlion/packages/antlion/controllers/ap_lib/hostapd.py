# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import itertools
import logging
import re
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from subprocess import CalledProcessError
from typing import Any, Iterable

from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.extended_capabilities import (
    ExtendedCapabilities,
)
from antlion.controllers.ap_lib.hostapd_config import HostapdConfig
from antlion.controllers.ap_lib.wireless_network_management import (
    BssTransitionManagementRequest,
)
from antlion.controllers.utils_lib.commands import shell
from antlion.logger import LogLevel
from honeydew.typing.custom_types import MacAddress
from libs.proc.runner import Runner
from tenacity import retry, retry_if_exception_type, stop_after_attempt

PROGRAM_FILE = "/usr/sbin/hostapd"
CLI_PROGRAM_FILE = "/usr/bin/hostapd_cli"


class Error(Exception):
    """An error caused by hostapd."""


class InterfaceInitError(Error):
    """Interface initialization failed during hostapd start."""


@dataclass
class StationTelemetry:
    """Represents RF and PHY telemetry of a station connected to an AP."""

    rssi: int | None = None
    tx_rate_mbps: float | None = None
    rx_rate_mbps: float | None = None
    snr: int | None = None
    phy_mode: str | None = None
    nss: int | None = None


@dataclass
class StationStatus:
    """Represents the connection status of a station on an AP."""

    auth: bool
    assoc: bool
    authorized: bool
    rssi: int | None = None
    tx_rate_mbps: float | None = None
    rx_rate_mbps: float | None = None
    snr: int | None = None
    phy_mode: str | None = None
    nss: int | None = None


def _parse_bitrate(val: float, unit: str) -> float:
    unit_upper = unit.upper()
    if "G" in unit_upper:
        return round(val * 1000.0, 2)
    elif "M" in unit_upper:
        return round(val, 2)
    elif "K" in unit_upper:
        return round(val / 1000.0, 2)
    else:
        return round(val / 1000000.0, 2)


def parse_station_output(output: object) -> StationTelemetry:
    """Parses station telemetry from station details (hostapd_cli or iw)."""
    if hasattr(output, "stdout"):
        output = output.stdout
    if isinstance(output, bytes):
        output = output.decode("utf-8", errors="replace")
    elif not isinstance(output, str):
        output = str(output)

    rssi: int | None = None
    tx_rate_mbps: float | None = None
    rx_rate_mbps: float | None = None

    m_sig = re.search(
        r"signal(?:\s+avg|_avg)?\s*[:=]\s*(-?\d+)", output, re.IGNORECASE
    )
    if m_sig:
        try:
            rssi = int(m_sig.group(1))
        except ValueError:
            pass

    m_tx = re.search(
        r"\btx[ _]?(?:bi?t?rate|rate)\s*[:=]\s*([0-9.]+)\s*([KMGT]?(?:Bit/s|bps|b/s))?",
        output,
        re.IGNORECASE,
    )
    if m_tx:
        try:
            val = float(m_tx.group(1))
            unit = m_tx.group(2)
            tx_rate_mbps = _parse_bitrate(val, unit) if unit else round(val, 2)
        except ValueError:
            pass

    if tx_rate_mbps is None:
        m_tx_info = re.search(
            r"tx_rate_info\s*[:=]\s*(\d+)", output, re.IGNORECASE
        )
        if m_tx_info:
            try:
                v = int(m_tx_info.group(1))
                if v > 0:
                    tx_rate_mbps = round(v / 10.0, 2)
            except ValueError:
                pass

    m_rx = re.search(
        r"\brx[ _]?(?:bi?t?rate|rate)\s*[:=]\s*([0-9.]+)\s*([KMGT]?(?:Bit/s|bps|b/s))?",
        output,
        re.IGNORECASE,
    )
    if m_rx:
        try:
            val = float(m_rx.group(1))
            unit = m_rx.group(2)
            rx_rate_mbps = _parse_bitrate(val, unit) if unit else round(val, 2)
        except ValueError:
            pass

    if rx_rate_mbps is None:
        m_rx_info = re.search(
            r"rx_rate_info\s*[:=]\s*(\d+)", output, re.IGNORECASE
        )
        if m_rx_info:
            try:
                v = int(m_rx_info.group(1))
                if v > 0:
                    rx_rate_mbps = round(v / 10.0, 2)
            except ValueError:
                pass

    snr: int | None = None
    m_snr = re.search(r"\bsnr\s*[:=]\s*(-?\d+)", output, re.IGNORECASE)
    if m_snr:
        try:
            snr = int(m_snr.group(1))
        except ValueError:
            pass
    elif rssi is not None:
        m_noise = re.search(r"\bnoise\s*[:=]\s*(-?\d+)", output, re.IGNORECASE)
        if m_noise:
            try:
                noise = int(m_noise.group(1))
                if noise < 0:
                    snr = rssi - noise
            except ValueError:
                pass

    phy_mode: str | None = None
    if re.search(r"\[EHT\]|\bEHT[-_]|EHT-MCS", output, re.IGNORECASE):
        phy_mode = "eht"
    elif re.search(r"\[HE\]|\bHE[-_]|HE-MCS", output, re.IGNORECASE):
        phy_mode = "he"
    elif re.search(r"\[VHT\]|\bVHT[-_]|VHT-MCS", output, re.IGNORECASE):
        phy_mode = "vht"
    elif re.search(
        r"\[HT\]|\bHT[-_]|HT-MCS|\bMCS\s*\d+", output, re.IGNORECASE
    ):
        phy_mode = "ht"

    nss: int | None = None
    m_nss = re.search(r"(?:[A-Z]+-)?NSS\s*[:= ]\s*(\d+)", output, re.IGNORECASE)
    if m_nss:
        try:
            nss = int(m_nss.group(1))
        except ValueError:
            pass
    if nss is None and phy_mode == "ht":
        m_mcs = re.search(r"\bMCS\s*[:= ]\s*(\d+)", output, re.IGNORECASE)
        if m_mcs:
            try:
                mcs = int(m_mcs.group(1))
                if mcs >= 0:
                    nss = (mcs // 8) + 1
            except ValueError:
                pass

    return StationTelemetry(
        rssi=rssi,
        tx_rate_mbps=tx_rate_mbps,
        rx_rate_mbps=rx_rate_mbps,
        snr=snr,
        phy_mode=phy_mode,
        nss=nss,
    )


class Hostapd(object):
    """Manages the hostapd program.

    Attributes:
        config: The hostapd configuration that is being used.
    """

    def __init__(
        self, runner: Runner, interface: str, working_dir: str = "/tmp"
    ) -> None:
        """
        Args:
            runner: Object that has run_async and run methods for executing
                    shell commands (e.g. connection.SshConnection)
            interface: The name of the interface to use (eg. wlan0).
            working_dir: The directory to work out of.
        """
        self._runner = runner
        self._interface = interface
        self._working_dir = working_dir
        self.config: HostapdConfig | None = None
        self._shell = shell.ShellCommand(runner)
        self._log_file = f"{working_dir}/hostapd-{self._interface}.log"
        self._ctrl_file = f"{working_dir}/hostapd-{self._interface}.ctrl"
        self._config_file = f"{working_dir}/hostapd-{self._interface}.conf"
        self._identifier = f"{PROGRAM_FILE}.*{self._config_file}"

    @retry(
        stop=stop_after_attempt(3),
        retry=retry_if_exception_type(InterfaceInitError),
    )
    def start(
        self,
        config: HostapdConfig,
        timeout: int = 60,
        additional_parameters: dict[str, Any] | None = None,
    ) -> None:
        """Starts hostapd

        Starts the hostapd daemon and runs it in the background.

        Args:
            config: Configs to start the hostapd with.
            timeout: Time to wait for DHCP server to come up.
            additional_parameters: A dictionary of parameters that can sent
                                   directly into the hostapd config file.  This
                                   can be used for debugging and or adding one
                                   off parameters into the config.

        Returns:
            True if the daemon could be started. Note that the daemon can still
            start and not work. Invalid configurations can take a long amount
            of time to be produced, and because the daemon runs indefinitely
            it's impossible to wait on. If you need to check if configs are ok
            then periodic checks to is_running and logs should be used.
        """
        if additional_parameters is None:
            additional_parameters = {}

        self.stop()

        self.config = config

        self._shell.delete_file(self._ctrl_file)
        self._shell.delete_file(self._log_file)
        self._shell.delete_file(self._config_file)
        self._write_configs(additional_parameters)

        hostapd_command = f'{PROGRAM_FILE} -dd -t "{self._config_file}"'
        base_command = f'cd "{self._working_dir}"; {hostapd_command}'
        job_str = (
            f'rfkill unblock all; {base_command} > "{self._log_file}" 2>&1'
        )
        self._runner.run_async(job_str)

        try:
            self._wait_for_process(timeout=timeout)
            self._wait_for_interface(timeout=timeout)
        except:
            self.stop()
            raise

    def stop(self) -> None:
        """Kills the daemon if it is running."""
        if self.is_alive():
            self._shell.kill(self._identifier)

    def channel_switch(self, channel_num: int, csa_beacon_count: int) -> None:
        """Switches to the given channel.

        Args:
            channel_num: Channel to switch to.
            csa_beacon_count: Number of channel switch announcement beacons to
                send.

        Returns:
            acts.libs.proc.job.Result containing the results of the command.

        Raises: See _run_hostapd_cli_cmd
        """
        try:
            channel_freq = hostapd_constants.FREQUENCY_MAP[channel_num]
        except KeyError:
            raise ValueError(f"Invalid channel number {channel_num}")
        channel_switch_cmd = f"chan_switch {csa_beacon_count} {channel_freq}"
        self._run_hostapd_cli_cmd(channel_switch_cmd)

    def get_current_channel(self) -> int:
        """Returns the current channel number.

        Raises: See _run_hostapd_cli_cmd
        """
        status_cmd = "status"
        result = self._run_hostapd_cli_cmd(status_cmd)
        match = re.search(r"^channel=(\d+)$", result, re.MULTILINE)
        if not match:
            raise Error("Current channel could not be determined")
        try:
            channel = int(match.group(1))
        except ValueError:
            raise Error("Internal error: current channel could not be parsed")
        return channel

    def get_stas(self) -> set[MacAddress]:
        """Return MAC addresses of all associated STAs."""
        list_sta_result = self._run_hostapd_cli_cmd("list_sta")
        stas = set()
        for line in list_sta_result.splitlines():
            # Each line must be a valid MAC address. Capture it.
            m = re.match(r"((?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2})", line)
            if m:
                stas.add(MacAddress(m.group(1)))
        return stas

    def _sta(self, sta_mac: MacAddress) -> str:
        """Return hostapd's detailed info about an associated STA.

        Returns:
            Results of the command.

        Raises: See _run_hostapd_cli_cmd
        """
        return self._run_hostapd_cli_cmd(f"sta {sta_mac}")

    def get_sta_extended_capabilities(
        self, sta_mac: MacAddress
    ) -> ExtendedCapabilities:
        """Get extended capabilities for the given STA, as seen by the AP.

        Args:
            sta_mac: MAC address of the STA in question.
        Returns:
            Extended capabilities of the given STA.
        Raises:
            Error if extended capabilities for the STA cannot be obtained.
        """
        sta_result = self._sta(sta_mac)
        # hostapd ext_capab field is a hex encoded string representation of the
        # 802.11 extended capabilities structure, each byte represented by two
        # chars (each byte having format %02x).
        m = re.search(r"ext_capab=([0-9A-Faf]+)", sta_result, re.MULTILINE)
        if not m:
            raise Error("Failed to get ext_capab from STA details")
        raw_ext_capab = m.group(1)
        try:
            return ExtendedCapabilities(bytearray.fromhex(raw_ext_capab))
        except ValueError:
            raise Error(
                f"ext_capab contains invalid hex string repr {raw_ext_capab}"
            )

    def sta_authenticated(self, sta_mac: MacAddress) -> bool:
        """Is the given STA authenticated?

        Args:
            sta_mac: MAC address of the STA in question.
        Returns:
            True if AP sees that the STA is authenticated, False otherwise.
        Raises:
            Error if authenticated status for the STA cannot be obtained.
        """
        sta_result = self._sta(sta_mac)
        m = re.search(r"flags=.*\[AUTH\]", sta_result, re.MULTILINE)
        return bool(m)

    def sta_associated(self, sta_mac: MacAddress) -> bool:
        """Is the given STA associated?

        Args:
            sta_mac: MAC address of the STA in question.
        Returns:
            True if AP sees that the STA is associated, False otherwise.
        Raises:
            Error if associated status for the STA cannot be obtained.
        """
        sta_result = self._sta(sta_mac)
        m = re.search(r"flags=.*\[ASSOC\]", sta_result, re.MULTILINE)
        return bool(m)

    def sta_authorized(self, sta_mac: MacAddress) -> bool:
        """Is the given STA authorized (802.1X controlled port open)?

        Args:
            sta_mac: MAC address of the STA in question.
        Returns:
            True if AP sees that the STA is 802.1X authorized, False otherwise.
        Raises:
            Error if authorized status for the STA cannot be obtained.
        """
        sta_result = self._sta(sta_mac)
        m = re.search(r"flags=.*\[AUTHORIZED\]", sta_result, re.MULTILINE)
        return bool(m)

    def get_sta_status(self, sta_mac: MacAddress) -> StationStatus:
        """Get station status for the given STA, as seen by hostapd.

        Args:
            sta_mac: MAC address of the STA in question.
        Returns:
            StationStatus for the given STA.
        Raises:
            Error if station status cannot be obtained.
        """
        sta_result = self._sta(sta_mac)
        if hasattr(sta_result, "stdout"):
            sta_result = sta_result.stdout
        if isinstance(sta_result, bytes):
            sta_result = sta_result.decode("utf-8", errors="replace")
        elif not isinstance(sta_result, str):
            sta_result = str(sta_result)

        auth = bool(re.search(r"flags=.*\[AUTH\]", sta_result, re.MULTILINE))
        assoc = bool(re.search(r"flags=.*\[ASSOC\]", sta_result, re.MULTILINE))
        authorized = bool(
            re.search(r"flags=.*\[AUTHORIZED\]", sta_result, re.MULTILINE)
        )
        telemetry = parse_station_output(sta_result)
        if (
            telemetry.rssi is None
            or telemetry.tx_rate_mbps is None
            or telemetry.rx_rate_mbps is None
        ):
            raise Error(
                f"Failed to obtain station telemetry for {sta_mac} from hostapd: "
                f"rssi={telemetry.rssi}, tx_rate_mbps={telemetry.tx_rate_mbps}, rx_rate_mbps={telemetry.rx_rate_mbps}"
            )
        return StationStatus(
            auth=auth,
            assoc=assoc,
            authorized=authorized,
            rssi=telemetry.rssi,
            tx_rate_mbps=telemetry.tx_rate_mbps,
            rx_rate_mbps=telemetry.rx_rate_mbps,
            snr=telemetry.snr,
            phy_mode=telemetry.phy_mode,
            nss=telemetry.nss,
        )

    def _bss_tm_req(
        self, client_mac: MacAddress, request: BssTransitionManagementRequest
    ) -> None:
        """Send a hostapd BSS Transition Management request command to a STA.

        Args:
            client_mac: MAC address that will receive the request.
            request: BSS Transition Management request that will be sent.
        Returns:
            acts.libs.proc.job.Result containing the results of the command.
        Raises: See _run_hostapd_cli_cmd
        """
        bss_tm_req_cmd = f"bss_tm_req {client_mac}"

        if request.abridged:
            bss_tm_req_cmd += " abridged=1"
        if (
            request.bss_termination_included
            and request.bss_termination_duration
        ):
            bss_tm_req_cmd += (
                f" bss_term={request.bss_termination_duration.duration}"
            )
        if request.disassociation_imminent:
            bss_tm_req_cmd += " disassoc_imminent=1"
        if request.disassociation_timer is not None:
            bss_tm_req_cmd += f" disassoc_timer={request.disassociation_timer}"
        if request.preferred_candidate_list_included:
            bss_tm_req_cmd += " pref=1"
        if request.session_information_url:
            bss_tm_req_cmd += f" url={request.session_information_url}"
        if request.validity_interval:
            bss_tm_req_cmd += f" valid_int={request.validity_interval}"

        # neighbor= can appear multiple times, so it requires special handling.
        if request.candidate_list is not None:
            for neighbor in request.candidate_list:
                bssid = neighbor.bssid
                bssid_info = hex(neighbor.bssid_information)
                op_class = neighbor.operating_class
                chan_num = neighbor.channel_number
                phy_type = int(neighbor.phy_type)
                bss_tm_req_cmd += f" neighbor={bssid},{bssid_info},{op_class},{chan_num},{phy_type}"

        self._run_hostapd_cli_cmd(bss_tm_req_cmd)

    def send_bss_transition_management_req(
        self, sta_mac: MacAddress, request: BssTransitionManagementRequest
    ) -> None:
        """Send a BSS Transition Management request to an associated STA.

        Args:
            sta_mac: MAC address of the STA in question.
            request: BSS Transition Management request that will be sent.
        Returns:
            acts.libs.proc.job.Result containing the results of the command.
        Raises: See _run_hostapd_cli_cmd
        """
        self._bss_tm_req(sta_mac, request)

    def is_alive(self) -> bool:
        """
        Returns:
            True if the daemon is running.
        """
        return self._shell.is_alive(self._identifier)

    def pull_logs(self) -> str:
        """Pulls the log files from where hostapd is running.

        Returns:
            A string of the hostapd logs.
        """
        # TODO: Auto pulling of logs when stop is called.
        with LogLevel(self._runner.log, logging.INFO):
            log = self._shell.read_file(self._log_file)

        # Convert epoch to human-readable times
        result: list[str] = []
        for line in log.splitlines():
            try:
                end = line.index(":")
                epoch = float(line[:end])
                timestamp = datetime.fromtimestamp(
                    epoch, timezone.utc
                ).strftime("%m-%d %H:%M:%S.%f")
                result.append(f"{timestamp} {line[end+1:]}")
            except ValueError:  # Colon not found or float conversion failure
                result.append(line)

        return "\n".join(result)

    def _run_hostapd_cli_cmd(self, cmd: str) -> str:
        """Run the given hostapd_cli command.

        Runs the command, waits for the output (up to default timeout), and
            returns the result.

        Returns:
            Results of the ssh command.

        Raises:
            subprocess.TimeoutExpired: When the remote command took too
                long to execute.
            libs.proc.runner.CalledProcessTransportError: When the ssh
                connection failed to be created.
            subprocess.CalledProcessError: Ssh worked, but the command had an
                error executing.
        """
        hostapd_cli_job = (
            f"cd {self._working_dir}; "
            f"{CLI_PROGRAM_FILE} -p {self._ctrl_file} {cmd}"
        )
        proc = self._runner.run(hostapd_cli_job)
        if proc.returncode:
            raise CalledProcessError(
                proc.returncode, hostapd_cli_job, proc.stdout, proc.stderr
            )
        return proc.stdout.decode("utf-8")

    def _wait_for_process(self, timeout: int = 60) -> None:
        """Waits for the process to come up.

        Waits until the hostapd process is found running, or there is
        a timeout. If the program never comes up then the log file
        will be scanned for errors.

        Raises: See _scan_for_errors
        """
        start_time = time.time()
        while time.time() - start_time < timeout and not self.is_alive():
            self._scan_for_errors(False)
            time.sleep(0.1)

    def _wait_for_interface(self, timeout: int = 60) -> None:
        """Waits for hostapd to report that the interface is up.

        Waits until hostapd says the interface has been brought up or an
        error occurs.

        Raises: see _scan_for_errors
        """
        start_time = time.time()
        while time.time() - start_time < timeout:
            time.sleep(0.1)
            success = self._shell.search_file(
                "Setup of interface done", self._log_file
            )
            if success:
                return
            self._scan_for_errors(False)

        self._scan_for_errors(True)

    def _scan_for_errors(self, should_be_up: bool) -> None:
        """Scans the hostapd log for any errors.

        Args:
            should_be_up: If true then hostapd program is expected to be alive.
                          If it is found not alive while this is true an error
                          is thrown.

        Raises:
            Error: when a hostapd error is found.
            InterfaceInitError: when the interface fails to initialize. This is
                a recoverable error that is usually caused by other processes
                using this interface at the same time.
        """
        # Store this so that all other errors have priority.
        is_dead = not self.is_alive()

        bad_config = self._shell.search_file(
            "Interface initialization failed", self._log_file
        )
        if bad_config:
            raise InterfaceInitError("Interface failed to initialize", self)

        bad_config = self._shell.search_file(
            f"Interface {self._interface} wasn't started", self._log_file
        )
        if bad_config:
            raise Error("Interface wasn't started", self)

        if should_be_up and is_dead:
            raise Error("Hostapd failed to start", self)

    def _write_configs(self, additional_parameters: dict[str, Any]) -> None:
        """Writes the configs to the hostapd config file."""
        self._shell.delete_file(self._config_file)

        interface_configs = collections.OrderedDict()
        interface_configs["interface"] = self._interface
        interface_configs["ctrl_interface"] = self._ctrl_file
        pairs: Iterable[str] = (
            f"{k}={v}" for k, v in interface_configs.items()
        )

        packaged_configs = self.config.package_configs() if self.config else []
        if additional_parameters:
            packaged_configs.append(additional_parameters)
        for packaged_config in packaged_configs:
            config_pairs = (
                f"{k}={v}" for k, v in packaged_config.items() if v is not None
            )
            pairs = itertools.chain(pairs, config_pairs)

        hostapd_conf = "\n".join(pairs)

        logging.info(f"Writing {self._config_file}")
        logging.debug("******************Start*******************")
        logging.debug(f"\n{hostapd_conf}")
        logging.debug("*******************End********************")

        self._shell.write_file(self._config_file, hostapd_conf)
