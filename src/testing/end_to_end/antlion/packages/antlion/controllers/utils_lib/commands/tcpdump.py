# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import time
from io import BufferedRandom
from pathlib import Path
from subprocess import Popen
from types import TracebackType

from antlion import utils
from libs.commands.command import LinuxCommand
from libs.proc.runner import Runner
from mobly.logger import (
    epoch_to_log_line_timestamp,
    normalize_log_line_timestamp,
)

# Max time to wait for tcpdump to terminate after sending SIGTERM.
TERMINATE_TIMEOUT_SEC: float = 5.0


class LinuxTcpdumpCommand(LinuxCommand):
    """Dump traffic on a network."""

    def __init__(self, runner: Runner, binary: str = "tcpdump") -> None:
        super().__init__(runner, binary)

    def start(self, interface: str, output_dir: Path) -> TcpdumpProcess:
        """Start tcpdump.

        Args:
            interface: Listen on this interface.
            path: Path to output directory

        Returns:
            A context manager to run tcpdump. Must be used in a with statement
            for the process to start and exit correctly.
        """
        time_stamp = normalize_log_line_timestamp(
            epoch_to_log_line_timestamp(utils.get_current_epoch_time())
        )
        return TcpdumpProcess(
            self, interface, pcap=Path(output_dir, f"tcpdump_{time_stamp}.pcap")
        )


class TcpdumpProcess:
    """Process running tcpdump."""

    def __init__(
        self,
        tcpdump: LinuxTcpdumpCommand,
        interface: str,
        pcap: Path,
    ) -> None:
        self._tcpdump = tcpdump
        self._log = tcpdump._runner.log
        self._interface = interface
        self._pcap_path = pcap
        self._pcap_file: BufferedRandom | None = None
        self._process: Popen[bytes] | None = None

    def __enter__(self) -> None:
        self._log.info(
            "Streaming %s packet capture to %s",
            self._interface,
            self._pcap_path,
        )
        self._pcap_file = self._pcap_path.open("w+b")
        self._process = self._tcpdump._start(
            [
                "-i",
                self._interface,
                # Stream pcap as bytes to stdout
                "-w",
                "-",
            ],
            sudo=True,
            stdout=self._pcap_file,
        )

    def __exit__(
        self,
        _exit_type: type[BaseException] | None,
        _exit_value: BaseException | None,
        _exit_traceback: TracebackType | None,
    ) -> None:
        if self._pcap_file is None or self._process is None:
            # tcpdump is not running.
            return

        self._process.terminate()
        timeout = time.time() + TERMINATE_TIMEOUT_SEC
        while time.time() < timeout:
            exit_code = self._process.poll()
            if exit_code is not None:
                self._pcap_file.close()
                self._pcap_file = None
                break
        else:
            self._process.kill()
            self._pcap_file.close()
            self._pcap_file = None
            raise TimeoutError(
                "tcpdump did not terminate after sending SIGTERM"
            )

        self._log.info(
            "%s packet capture wrote to %s", self._interface, self._pcap_path
        )

        _, stderr = self._process.communicate()
        self._log.debug(
            "tcpdump returned with status %i\nstderr: %s",
            exit_code,
            stderr.decode("utf-8", errors="replace"),
        )
