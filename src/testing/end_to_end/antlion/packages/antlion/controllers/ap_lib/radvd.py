# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import shlex
import tempfile
import time

from antlion.controllers.ap_lib.radvd_config import RadvdConfig
from antlion.controllers.utils_lib.commands import shell
from antlion.logger import LogLevel
from libs.proc import job
from libs.proc.runner import Runner
from tenacity import retry, retry_if_exception_type, stop_after_delay


class RadvdStartError(Exception):
    """Radvd failed to start."""


class Radvd(object):
    """Manages the radvd program.

    https://en.wikipedia.org/wiki/Radvd
    This implements the Router Advertisement Daemon of IPv6 router addresses
    and IPv6 routing prefixes using the Neighbor Discovery Protocol.

    Attributes:
        config: The radvd configuration that is being used.
    """

    def __init__(
        self,
        runner: Runner,
        interface: str,
        working_dir: str | None = None,
        radvd_binary: str | None = None,
    ) -> None:
        """
        Args:
            runner: Object that has run_async and run methods for executing
                    shell commands (e.g. connection.SshConnection)
            interface: Name of the interface to use (eg. wlan0).
            working_dir: Directory to work out of.
            radvd_binary: Location of the radvd binary
        """
        if not radvd_binary:
            logging.debug(
                "No radvd binary specified.  " "Assuming radvd is in the path."
            )
            radvd_binary = "radvd"
        else:
            logging.debug(f"Using radvd binary located at {radvd_binary}")
        if working_dir is None and runner.run == job.run:
            working_dir = tempfile.gettempdir()
        else:
            working_dir = "/tmp"
        self._radvd_binary = radvd_binary
        self._runner = runner
        self._interface = interface
        self._working_dir = working_dir
        self.config: RadvdConfig | None = None
        self._shell = shell.ShellCommand(runner)
        self._log_file = f"{working_dir}/radvd-{self._interface}.log"
        self._config_file = f"{working_dir}/radvd-{self._interface}.conf"
        self._pid_file = f"{working_dir}/radvd-{self._interface}.pid"
        self._ps_identifier = f"{self._radvd_binary}.*{self._config_file}"

    def start(self, config: RadvdConfig) -> None:
        """Starts radvd

        Starts the radvd daemon and runs it in the background.

        Args:
            config: Configs to start the radvd with.

        Returns:
            True if the daemon could be started. Note that the daemon can still
            start and not work. Invalid configurations can take a long amount
            of time to be produced, and because the daemon runs indefinitely
            it's impossible to wait on. If you need to check if configs are ok
            then periodic checks to is_running and logs should be used.

        Raises:
            RadvdStartError: when a radvd error is found or process is dead
        """
        if self.is_alive():
            self.stop()

        self.config = config

        self._shell.delete_file(self._log_file)
        self._shell.delete_file(self._config_file)
        self._write_configs(self.config)

        try:
            self._launch()
        except RadvdStartError:
            self.stop()
            raise

    # TODO(http://b/372534563): Remove retries once the source of SIGINT is
    # found and a fix is implemented.
    @retry(
        stop=stop_after_delay(30),
        retry=retry_if_exception_type(RadvdStartError),
    )
    def _launch(self) -> None:
        """Launch the radvd process with retries.

        Raises:
            RadvdStartError: when a radvd error is found or process is dead
        """
        command = (
            f"{self._radvd_binary} -C {shlex.quote(self._config_file)} "
            f"-p {shlex.quote(self._pid_file)} -m logfile -d 5 "
            f'-l {self._log_file} > "{self._log_file}" 2>&1'
        )
        self._runner.run_async(command)
        self._wait_for_process(timeout=10)

    def stop(self) -> None:
        """Kills the daemon if it is running."""
        self._shell.kill(self._ps_identifier)

    def is_alive(self) -> bool:
        """
        Returns:
            True if the daemon is running.
        """
        return self._shell.is_alive(self._ps_identifier)

    def pull_logs(self) -> str:
        """Pulls the log files from where radvd is running.

        Returns:
            A string of the radvd logs.
        """
        # TODO: Auto pulling of logs when stop is called.
        with LogLevel(self._runner.log, logging.INFO):
            return self._shell.read_file(self._log_file)

    def _wait_for_process(self, timeout: int = 60) -> None:
        """Waits for the process to come up.

        Waits until the radvd process is found running, or there is
        a timeout. If the program never comes up then the log file
        will be scanned for errors.

        Raises:
            RadvdStartError: when a radvd error is found or process is dead
        """
        start_time = time.time()
        while time.time() - start_time < timeout and not self.is_alive():
            time.sleep(0.1)
            self._scan_for_errors(False)
        self._scan_for_errors(True)

    def _scan_for_errors(self, should_be_up: bool) -> None:
        """Scans the radvd log for any errors.

        Args:
            should_be_up: If true then radvd program is expected to be alive.
                          If it is found not alive while this is true an error
                          is thrown.

        Raises:
            RadvdStartError: when a radvd error is found or process is dead
        """
        # Store this so that all other errors have priority.
        is_dead = not self.is_alive()

        exited_prematurely = self._shell.search_file("Exiting", self._log_file)
        if exited_prematurely:
            raise RadvdStartError("Radvd exited prematurely.", self)
        if should_be_up and is_dead:
            raise RadvdStartError("Radvd failed to start", self)

    def _write_configs(self, config: RadvdConfig) -> None:
        """Writes the configs to the radvd config file.

        Args:
            config: a RadvdConfig object.
        """
        self._shell.delete_file(self._config_file)
        conf = config.package_configs()
        lines = ["interface %s {" % self._interface]
        for interface_option_key, interface_option in conf[
            "interface_options"
        ].items():
            lines.append(
                f"\t{str(interface_option_key)} {str(interface_option)};"
            )
        lines.append(f"\tprefix {conf['prefix']}")
        lines.append("\t{")
        for prefix_option in conf["prefix_options"].items():
            lines.append(f"\t\t{' '.join(map(str, prefix_option))};")
        lines.append("\t};")
        if conf["clients"]:
            lines.append("\tclients")
            lines.append("\t{")
            for client in conf["clients"]:
                lines.append(f"\t\t{client};")
            lines.append("\t};")
        if conf["route"]:
            lines.append("\troute %s {" % conf["route"])
            for route_option in conf["route_options"].items():
                lines.append(f"\t\t{' '.join(map(str, route_option))};")
            lines.append("\t};")
        if conf["rdnss"]:
            lines.append(
                "\tRDNSS %s {" % " ".join([str(elem) for elem in conf["rdnss"]])
            )
            for rdnss_option in conf["rdnss_options"].items():
                lines.append(f"\t\t{' '.join(map(str, rdnss_option))};")
            lines.append("\t};")
        lines.append("};")
        output_config = "\n".join(lines)
        logging.info(f"Writing {self._config_file}")
        logging.debug("******************Start*******************")
        logging.debug(f"\n{output_config}")
        logging.debug("*******************End********************")

        self._shell.write_file(self._config_file, output_config)
