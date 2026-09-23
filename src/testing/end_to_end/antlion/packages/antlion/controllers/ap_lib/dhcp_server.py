# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import time

from antlion.controllers.ap_lib.dhcp_config import DhcpConfig
from antlion.controllers.utils_lib.commands import shell
from libs.proc.runner import Runner
from mobly import logger
from tenacity import (
    retry,
    retry_if_exception_type,
    stop_after_attempt,
    wait_fixed,
)


class Error(Exception):
    """An error caused by the dhcp server."""


class NoInterfaceError(Exception):
    """Error thrown when the dhcp server has no interfaces on any subnet."""


class DhcpServer(object):
    """Manages the dhcp server program.

    Only one of these can run in an environment at a time.

    Attributes:
        config: The dhcp server configuration that is being used.
    """

    PROGRAM_FILE = "dhcpd"

    def __init__(
        self, runner: Runner, interface: str, working_dir: str = "/tmp"
    ):
        """
        Args:
            runner: Object that has a run_async and run methods for running
                    shell commands.
            interface: string, The name of the interface to use.
            working_dir: The directory to work out of.
        """
        self._log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[DHCP Server|{interface}]",
            },
        )

        self._runner = runner
        self._working_dir = working_dir
        self._shell = shell.ShellCommand(runner)
        self._stdio_log_file = f"{working_dir}/dhcpd_{interface}.log"
        self._config_file = f"{working_dir}/dhcpd_{interface}.conf"
        self._lease_file = f"{working_dir}/dhcpd_{interface}.leases"
        self._pid_file = f"{working_dir}/dhcpd_{interface}.pid"
        # Identify the daemon process by command pattern rather than the
        # subshell PID returned by run_async, allowing ShellCommand to track and
        # terminate it via ps.
        self._identifier = f"{self.PROGRAM_FILE}.*{self._config_file}"

    # There is a slight timing issue where if the proc filesystem in Linux
    # doesn't get updated in time as when this is called, the NoInterfaceError
    # will happening.  By adding this retry, the error appears to have gone away
    # but will still show a warning if the problem occurs.  The error seems to
    # happen more with bridge interfaces than standard interfaces.
    @retry(
        retry=retry_if_exception_type(NoInterfaceError),
        stop=stop_after_attempt(3),
        wait=wait_fixed(1),
    )
    def start(self, config: DhcpConfig, timeout_sec: int = 60) -> None:
        """Starts the dhcp server.

        Starts the dhcp server daemon and runs it in the background.

        Args:
            config: Configs to start the dhcp server with.

        Raises:
            Error: Raised when a dhcp server error is found.
        """
        if self.is_alive():
            self.stop()

        self._write_configs(config)
        self._shell.delete_file(self._stdio_log_file)
        self._shell.delete_file(self._pid_file)
        self._shell.touch_file(self._lease_file)

        dhcpd_command = (
            f"{self.PROGRAM_FILE} "
            f'-cf "{self._config_file}" '
            f"-lf {self._lease_file} "
            f'-pf "{self._pid_file}" '
            "-f -d"
        )

        base_command = f'cd "{self._working_dir}"; {dhcpd_command}'
        job_str = f'{base_command} > "{self._stdio_log_file}" 2>&1'
        self._runner.run_async(job_str)

        try:
            self._wait_for_process(timeout=timeout_sec)
            self._wait_for_server(timeout=timeout_sec)
        except:
            self._log.warning("Failed to start DHCP server.")
            self._log.info(
                f"DHCP configuration:\n{config.render_config_file()}\n"
            )
            self._log.info(f"DHCP logs:\n{self.get_logs()}\n")
            self.stop()
            raise

    def stop(self) -> None:
        """Kills the daemon if it is running."""
        if self.is_alive():
            self._shell.kill(self._identifier)

    def is_alive(self) -> bool:
        """
        Returns:
            True if the daemon is running.
        """
        return self._shell.is_alive(self._identifier)

    def get_logs(self) -> str:
        """Pulls the log files from where dhcp server is running.

        Returns:
            A string of the dhcp server logs.
        """
        return self._shell.read_file(self._stdio_log_file)

    def _wait_for_process(self, timeout: float = 60) -> None:
        """Waits for the process to come up.

        Waits until the dhcp server process is found running, or there is
        a timeout. If the program never comes up then the log file
        will be scanned for errors.

        Raises: See _scan_for_errors
        """
        start_time = time.time()
        while time.time() - start_time < timeout and not self.is_alive():
            self._scan_for_errors(False)
            time.sleep(0.1)

        self._scan_for_errors(True)

    def _wait_for_server(self, timeout: float = 60) -> None:
        """Waits for dhcp server to report that the server is up.

        Waits until dhcp server says the server has been brought up or an
        error occurs.

        Raises: see _scan_for_errors
        """
        start_time = time.time()
        while time.time() - start_time < timeout:
            success = self._shell.search_file(
                "Wrote [0-9]* leases to leases file", self._stdio_log_file
            )
            if success:
                return

            self._scan_for_errors(True)

    def _scan_for_errors(self, should_be_up: bool) -> None:
        """Scans the dhcp server log for any errors.

        Args:
            should_be_up: If true then dhcp server is expected to be alive.
                          If it is found not alive while this is true an error
                          is thrown.

        Raises:
            Error: Raised when a dhcp server error is found.
        """
        # If this is checked last we can run into a race condition where while
        # scanning the log the process has not died, but after scanning it
        # has. If this were checked last in that condition then the wrong
        # error will be thrown. To prevent this we gather the alive state first
        # so that if it is dead it will definitely give the right error before
        # just giving a generic one.
        is_dead = not self.is_alive()

        no_interface = self._shell.search_file(
            "Not configured to listen on any interfaces", self._stdio_log_file
        )
        if no_interface:
            raise NoInterfaceError(
                "Dhcp does not contain a subnet for any of the networks the"
                " current interfaces are on."
            )

        if should_be_up and is_dead:
            raise Error("Dhcp server failed to start.", self)

    def _write_configs(self, config: DhcpConfig) -> None:
        """Writes the configs to the dhcp server config file."""
        self._shell.delete_file(self._config_file)
        config_str = config.render_config_file()
        self._shell.write_file(self._config_file, config_str)
