#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from __future__ import annotations

import json
import logging
import math
import os
import re
import shlex
import subprocess
import threading
import time
from typing import IO

from antlion import context, utils
from antlion.types import ControllerConfig, Json
from antlion.validation import MapValidator
from libs.commands import nmcli
from libs.commands.command import (
    LinuxCommand,
    optional,
)
from libs.commands.journalctl import (
    LinuxJournalctlCommand,
)
from libs.proc import job
from libs.ssh import connection, settings
from mobly import logger, signals

MOBLY_CONTROLLER_CONFIG_NAME: str = "IPerfServer"
KILOBITS = 1024
MEGABITS = KILOBITS * 1024
GIGABITS = MEGABITS * 1024
BITS_IN_BYTE = 8


def create(
    configs: list[ControllerConfig],
) -> list[IPerfServer | IPerfServerOverSsh]:
    """Factory method for iperf servers.

    The function creates iperf servers based on at least one config.
    If configs only specify a port number, a regular local IPerfServer object
    will be created. If configs contains ssh settings or and AndroidDevice,
    remote iperf servers will be started on those devices

    Args:
        configs: config parameters for the iperf server
    """
    results: list[IPerfServer | IPerfServerOverSsh] = []
    for c in configs:
        if isinstance(c, (str, int)) and str(c).isdigit():
            results.append(IPerfServer(int(c)))
        elif isinstance(c, dict) and "ssh_config" in c and "port" in c:
            config = MapValidator(c)
            results.append(
                IPerfServerOverSsh(
                    settings.from_config(config.get(dict, "ssh_config")),
                    config.get(int, "port"),
                    test_interface=config.get(str, "test_interface"),
                    use_killall=config.get(bool, "use_killall", False),
                )
            )
        else:
            raise ValueError(
                f"Config entry {c} in {configs} is not a valid IPerfServer config."
            )
    return results


def destroy(
    objects: list[IPerfServer | IPerfServerOverSsh],
) -> None:
    for iperf_server in objects:
        try:
            iperf_server.stop()
        except Exception:
            logging.exception(f"Unable to properly clean up {iperf_server}.")


def get_info(
    objects: list[IPerfServer | IPerfServerOverSsh],
) -> list[Json]:
    return []


class IPerfResult(object):
    def __init__(self, result_path, reporting_speed_units="Mbytes"):
        """Loads iperf result from file.

        Loads iperf result from JSON formatted server log. File can be accessed
        before or after server is stopped. Note that only the first JSON object
        will be loaded and this funtion is not intended to be used with files
        containing multiple iperf client runs.
        """
        # if result_path isn't a path, treat it as JSON
        self.reporting_speed_units = reporting_speed_units
        if not os.path.exists(result_path):
            self.result = json.loads(result_path)
        else:
            try:
                with open(result_path, "r") as f:
                    iperf_output = f.readlines()
                    if "}\n" in iperf_output:
                        iperf_output = iperf_output[
                            : iperf_output.index("}\n") + 1
                        ]
                    iperf_string = "".join(iperf_output)
                    iperf_string = iperf_string.replace("nan", "0")
                    self.result = json.loads(iperf_string)
            except ValueError:
                with open(result_path, "r") as f:
                    # Possibly a result from interrupted iperf run,
                    # skip first line and try again.
                    lines = f.readlines()[1:]
                    self.result = json.loads("".join(lines))

    def _has_data(self):
        """Checks if the iperf result has valid throughput data.

        Returns:
            True if the result contains throughput data. False otherwise.
        """
        return ("end" in self.result) and (
            "sum_received" in self.result["end"] or "sum" in self.result["end"]
        )

    def _get_reporting_speed(
        self, network_speed_in_bits_per_second: int | float
    ) -> float:
        """Sets the units for the network speed reporting based on how the
        object was initiated.  Defaults to Megabytes per second.  Currently
        supported, bits per second (bits), kilobits per second (kbits), megabits
        per second (mbits), gigabits per second (gbits), bytes per second
        (bytes), kilobits per second (kbytes), megabits per second (mbytes),
        gigabytes per second (gbytes).

        Args:
            network_speed_in_bits_per_second: The network speed from iperf in
                bits per second.

        Returns:
            The value of the throughput in the appropriate units.
        """
        speed_divisor = 1
        if self.reporting_speed_units[1:].lower() == "bytes":
            speed_divisor = speed_divisor * BITS_IN_BYTE
        if self.reporting_speed_units[0:1].lower() == "k":
            speed_divisor = speed_divisor * KILOBITS
        if self.reporting_speed_units[0:1].lower() == "m":
            speed_divisor = speed_divisor * MEGABITS
        if self.reporting_speed_units[0:1].lower() == "g":
            speed_divisor = speed_divisor * GIGABITS
        return network_speed_in_bits_per_second / speed_divisor

    def get_json(self):
        """Returns the raw json output from iPerf."""
        return self.result

    @property
    def error(self):
        return self.result.get("error", None)

    @property
    def avg_rate(self):
        """Average UDP rate in MB/s over the entire run.

        This is the average UDP rate observed at the terminal the iperf result
        is pulled from. According to iperf3 documentation this is calculated
        based on bytes sent and thus is not a good representation of the
        quality of the link. If the result is not from a success run, this
        property is None.
        """
        if not self._has_data() or "sum" not in self.result["end"]:
            return None
        bps = self.result["end"]["sum"]["bits_per_second"]
        return self._get_reporting_speed(bps)

    @property
    def avg_receive_rate(self):
        """Average receiving rate in MB/s over the entire run.

        This data may not exist if iperf was interrupted. If the result is not
        from a success run, this property is None.
        """
        if not self._has_data() or "sum_received" not in self.result["end"]:
            return None
        bps = self.result["end"]["sum_received"]["bits_per_second"]
        return self._get_reporting_speed(bps)

    @property
    def avg_send_rate(self):
        """Average sending rate in MB/s over the entire run.

        This data may not exist if iperf was interrupted. If the result is not
        from a success run, this property is None.
        """
        if not self._has_data() or "sum_sent" not in self.result["end"]:
            return None
        bps = self.result["end"]["sum_sent"]["bits_per_second"]
        return self._get_reporting_speed(bps)

    @property
    def instantaneous_rates(self):
        """Instantaneous received rate in MB/s over entire run.

        This data may not exist if iperf was interrupted. If the result is not
        from a success run, this property is None.
        """
        if not self._has_data():
            return None
        intervals = [
            self._get_reporting_speed(interval["sum"]["bits_per_second"])
            for interval in self.result["intervals"]
        ]
        return intervals

    @property
    def std_deviation(self):
        """Standard deviation of rates in MB/s over entire run.

        This data may not exist if iperf was interrupted. If the result is not
        from a success run, this property is None.
        """
        return self.get_std_deviation(0)

    def get_std_deviation(self, iperf_ignored_interval):
        """Standard deviation of rates in MB/s over entire run.

        This data may not exist if iperf was interrupted. If the result is not
        from a success run, this property is None. A configurable number of
        beginning (and the single last) intervals are ignored in the
        calculation as they are inaccurate (e.g. the last is from a very small
        interval)

        Args:
            iperf_ignored_interval: number of iperf interval to ignored in
            calculating standard deviation

        Returns:
            The standard deviation.
        """
        if not self._has_data():
            return None
        instantaneous_rates = self.instantaneous_rates[
            iperf_ignored_interval:-1
        ]
        avg_rate = math.fsum(instantaneous_rates) / len(instantaneous_rates)
        sqd_deviations = [
            (rate - avg_rate) ** 2 for rate in instantaneous_rates
        ]
        std_dev = math.sqrt(
            math.fsum(sqd_deviations) / (len(sqd_deviations) - 1)
        )
        return std_dev


class IPerfServerBase(object):
    # Keeps track of the number of IPerfServer logs to prevent file name
    # collisions.
    __log_file_counter = 0

    __log_file_lock = threading.Lock()

    def __init__(self, port: int):
        self._port = port
        # TODO(markdr): We shouldn't be storing the log files in an array like
        # this. Nobody should be reading this property either. Instead, the
        # IPerfResult should be returned in stop() with all the necessary info.
        # See aosp/1012824 for a WIP implementation.
        self.log_files: list[str] = []

    @property
    def port(self) -> int:
        raise NotImplementedError("port must be specified.")

    @property
    def started(self) -> bool:
        raise NotImplementedError("started must be specified.")

    def start(self, extra_args: str = "", tag: str = "") -> None:
        """Starts an iperf3 server.

        Args:
            extra_args: Extra arguments to start iperf server with.
            tag: Appended to log file name to identify logs from different
                iperf runs.
        """
        raise NotImplementedError("start() must be specified.")

    def stop(self) -> str | None:
        """Stops the iperf server.

        Returns:
            The name of the log file generated from the terminated session, or
            None if iperf wasn't started or ran successfully.
        """
        raise NotImplementedError("stop() must be specified.")

    def _get_full_file_path(self, tag: str | None = None) -> str:
        """Returns the full file path for the IPerfServer log file.

        Note: If the directory for the file path does not exist, it will be
        created.

        Args:
            tag: The tag passed in to the server run.
        """
        out_dir = self.log_path

        with IPerfServerBase.__log_file_lock:
            tags = [tag, IPerfServerBase.__log_file_counter]
            out_file_name = "IPerfServer,%s.log" % (
                ",".join([str(x) for x in tags if x != "" and x is not None])
            )
            IPerfServerBase.__log_file_counter += 1

        file_path = os.path.join(out_dir, out_file_name)
        self.log_files.append(file_path)
        return file_path

    @property
    def log_path(self) -> str:
        current_context = context.get_current_context()
        full_out_dir = os.path.join(
            current_context.get_full_output_path(), f"IPerfServer{self.port}"
        )

        # Ensure the directory exists.
        os.makedirs(full_out_dir, exist_ok=True)

        return full_out_dir


def _get_port_from_ss_output(ss_output, pid):
    pid = str(pid)
    lines = ss_output.split("\n")
    for line in lines:
        if pid in line:
            # Expected format:
            # tcp LISTEN  0 5 *:<PORT>  *:* users:(("cmd",pid=<PID>,fd=3))
            return line.split()[4].split(":")[-1]
    else:
        raise ProcessLookupError("Could not find started iperf3 process.")


class IPerfServer(IPerfServerBase):
    """Class that handles iperf server commands on localhost."""

    def __init__(self, port: int = 5201) -> None:
        super().__init__(port)
        self._hinted_port = port
        self._current_log_file: str | None = None
        self._iperf_process: subprocess.Popen[bytes] | None = None
        self._last_opened_file: IO[bytes] | None = None

    @property
    def port(self) -> int:
        return self._port

    @property
    def started(self) -> bool:
        return self._iperf_process is not None

    def start(self, extra_args: str = "", tag: str = "") -> None:
        """Starts iperf server on local machine.

        Args:
            extra_args: A string representing extra arguments to start iperf
                server with.
            tag: Appended to log file name to identify logs from different
                iperf runs.
        """
        if self._iperf_process is not None:
            return

        self._current_log_file = self._get_full_file_path(tag)

        # Run an iperf3 server on the hinted port with JSON output.
        command = ["iperf3", "-s", "-p", str(self._hinted_port), "-J"]

        command.extend(shlex.split(extra_args))

        if self._last_opened_file:
            self._last_opened_file.close()
        self._last_opened_file = open(self._current_log_file, "wb")
        self._iperf_process = subprocess.Popen(
            command, stdout=self._last_opened_file, stderr=subprocess.DEVNULL
        )
        for attempts_left in reversed(range(3)):
            try:
                self._port = int(
                    _get_port_from_ss_output(
                        job.run("ss -l -p -n | grep iperf").stdout,
                        self._iperf_process.pid,
                    )
                )
                break
            except ProcessLookupError:
                if attempts_left == 0:
                    raise
                logging.debug("iperf3 process not started yet.")
                time.sleep(0.01)

    def stop(self) -> str | None:
        """Stops the iperf server.

        Returns:
            The name of the log file generated from the terminated session, or
            None if iperf wasn't started or ran successfully.
        """
        if self._iperf_process is None:
            return None

        if self._last_opened_file:
            self._last_opened_file.close()
            self._last_opened_file = None

        self._iperf_process.terminate()
        self._iperf_process = None

        return self._current_log_file

    def __del__(self) -> None:
        self.stop()


class IPerfServerOverSsh(IPerfServerBase):
    """Class that handles iperf3 operations on remote machines."""

    def __init__(
        self,
        ssh_settings: settings.SshSettings,
        port: int,
        test_interface: str,
        use_killall: bool = False,
    ):
        super().__init__(port)
        self.test_interface = test_interface
        self.hostname = ssh_settings.hostname
        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[IPerfServer | {self.hostname}]",
            },
        )
        self._ssh_settings = ssh_settings
        self._ssh_session: connection.SshConnection | None = (
            connection.SshConnection(ssh_settings)
        )
        self._journalctl = optional(LinuxJournalctlCommand(self._ssh_session))
        self._ss = optional(LinuxCommand(self._ssh_session, "ss"))

        self._iperf_pid: str | None = None
        self._current_tag: str | None = None
        self._use_killall = str(use_killall).lower() == "true"

        # Disable NetworkManager on the test interface
        self._nmcli = optional(nmcli.LinuxNmcliCommand(self._ssh_session))
        if self._nmcli:
            # The control and test interfaces have to be different, otherwise
            # performing a DHCP release+renewal risks severing the SSH connection
            # and bricking the device.
            control_interface = utils.get_interface_based_on_ip(
                self._ssh_session, self.hostname
            )
            if control_interface == test_interface:
                raise signals.TestAbortAll(
                    f"iperf server control interface ({control_interface}) cannot be the "
                    f"same as the test interface ({test_interface})."
                )
            self._nmcli.setup_device(self.test_interface)

    @property
    def port(self) -> int:
        return self._port

    @property
    def started(self) -> bool:
        return self._iperf_pid is not None

    def _get_remote_log_path(self) -> str:
        return f"/tmp/iperf_server_port{self.port}.log"

    def get_interface_ip_addresses(
        self, interface: str
    ) -> dict[str, list[str]]:
        """Gets all of the ip addresses, ipv4 and ipv6, associated with a
           particular interface name.

        Args:
            interface: The interface name on the device, ie eth0

        Returns:
            A list of dictionaries of the various IP addresses. See
            utils.get_interface_ip_addresses.
        """
        return utils.get_interface_ip_addresses(self._get_ssh(), interface)

    def renew_test_interface_ip_address(self) -> None:
        """Renews the test interface's IPv4 address.

        Necessary for changing DHCP scopes during a test.
        """
        utils.renew_linux_ip_address(self._get_ssh(), self.test_interface)

    def get_addr(
        self, addr_type: str = "ipv4_private", timeout_sec: int | None = None
    ) -> str:
        """Wait until a type of IP address on the test interface is available
        then return it.
        """
        return utils.get_addr(
            self._get_ssh(), self.test_interface, addr_type, timeout_sec
        )

    def _cleanup_iperf_port(self) -> None:
        """Checks and kills zombie iperf servers occupying intended port."""
        assert self._ssh_session is not None
        if not self._ss:
            if self._use_killall:
                self._ssh_session.run("killall iperf3", ignore_status=True)
            else:
                raise Exception(
                    "Cannot clean up iperf port: 'ss' tool is unavailable on the remote host, and fallback 'killall' is disabled."
                )
            return

        ss_output = self._ssh_session.run(["ss", "-tupln"]).stdout.decode(
            "utf-8"
        )
        for line in ss_output.splitlines():
            if (
                "LISTEN" in line
                and "iperf3" in line
                and f":{self.port}" in line
            ):
                match = re.search(r"pid=(\d+)", line)
                if match:
                    pid = int(match.group(1))
                else:
                    match = re.search(r'"iperf3",(\d+),', line)
                    if match:
                        pid = int(match.group(1))
                    else:
                        continue
                logging.debug(
                    "Killing zombie server on port %i: %i", self.port, pid
                )
                self._ssh_session.run(["kill", "-9", str(pid)])

    def start(
        self,
        extra_args: str = "",
        tag: str = "",
        iperf_binary: str | None = None,
    ) -> None:
        """Starts iperf server on specified machine and port.

        Args:
            extra_args: Extra arguments to start iperf server with.
            tag: Appended to log file name to identify logs from different
                iperf runs.
            iperf_binary: Location of iperf3 binary. If none, it is assumed the
                the binary is in the path.
        """
        if self.started:
            return

        self._cleanup_iperf_port()
        if not iperf_binary:
            logging.debug(
                "No iperf3 binary specified.  "
                "Assuming iperf3 is in the path."
            )
            iperf_binary = "iperf3"
        else:
            logging.debug(f"Using iperf3 binary located at {iperf_binary}")
        iperf_command = f"{iperf_binary} -s -J -p {self.port}"

        cmd = f"{iperf_command} {extra_args} > {self._get_remote_log_path()}"

        job_result = self._get_ssh().run_async(cmd)
        self._iperf_pid = job_result.stdout.decode("utf-8")
        self._current_tag = tag

    def stop(self) -> str | None:
        """Stops the iperf server.

        Returns:
            The name of the log file generated from the terminated session, or
            None if iperf wasn't started or ran successfully.
        """
        if not self.started:
            return None

        ssh = self._get_ssh()

        if self._use_killall:
            ssh.run(["killall", "iperf3"], ignore_status=True)
        elif self._iperf_pid:
            ssh.run(["kill", "-9", self._iperf_pid])

        iperf_result = ssh.run(f"cat {self._get_remote_log_path()}")

        log_file = self._get_full_file_path(self._current_tag)
        with open(log_file, "wb") as f:
            f.write(iperf_result.stdout)

        ssh.run(["rm", self._get_remote_log_path()])
        self._iperf_pid = None
        return log_file

    def _get_ssh(self) -> connection.SshConnection:
        if self._ssh_session is None:
            self._ssh_session = connection.SshConnection(self._ssh_settings)

            # Disable NetworkManager on the test interface
            self._nmcli = optional(nmcli.LinuxNmcliCommand(self._ssh_session))
            if self._nmcli:
                self._nmcli.setup_device(self.test_interface)

        return self._ssh_session

    def close_ssh(self) -> None:
        """Closes the ssh session to the iperf server, if one exists, preventing
        connection reset errors when rebooting server device.
        """
        if self.started:
            self.stop()
        if self._ssh_session:
            self._ssh_session.close()
            self._ssh_session = None

    def get_systemd_journal(self) -> str:
        if not self._journalctl:
            return "journalctl not available"

        had_ssh = False if self._ssh_session is None else True

        self._journalctl.set_runner(self._get_ssh())
        logs = self._journalctl.logs()

        if not had_ssh:
            # Return to closed state
            self.close_ssh()

        return logs

    def download_logs(self, path: str) -> None:
        """Download all available logs to path.

        Args:
            path: Path to write logs to.
        """
        timestamp = logger.normalize_log_line_timestamp(
            logger.epoch_to_log_line_timestamp(utils.get_current_epoch_time())
        )

        systemd_journal = self.get_systemd_journal()
        systemd_journal_path = os.path.join(
            path, f"iperf_systemd_{timestamp}.log"
        )
        with open(systemd_journal_path, "a") as f:
            f.write(systemd_journal)
        self.log.info(f"Wrote systemd journal to {systemd_journal_path}")
