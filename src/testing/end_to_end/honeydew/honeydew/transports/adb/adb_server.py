# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Provides a wrapper for running an isolated adb server process."""

import atexit
import logging
import os
import socket
import subprocess
import tarfile
import tempfile
import threading
import time
from pathlib import Path

from honeydew.transports.adb import errors as adb_errors

_LOGGER: logging.Logger = logging.getLogger(__name__)


class AdbServer:
    """Wrapper for running an isolated adb server process.

    The adb server runs on the host and only allows connection to a single USB device.

    The number of TCP devices the server allows is not limited.

    Running a dedicated, isolated server in each test helps isolate the test
    from other tests and devices in the environment. When sharing the server,
    a badly behaving test or device can crash the server and affect unrelated
    tests and devices.

    adb clients should use the -H (host) and -P (port) arguments to connect to
    this host.
    """

    # adb server host address must be an IP address rather than "localhost".
    # Otherwise, adb client commands try to start their own local server
    # if they cannot already find one. However, we want them to instead connect
    # to the server started by "adb server nodaemon".
    _SERVER_HOST_IP = "127.0.0.1"

    _MIN_ADB_PORT_NUMBER = 50000
    _MAX_ADB_PORT_NUMBER = 60000

    # When the connection between the locally-running adb server and adbd on the device
    # enters a bad state, restarting one or both allows the test to continue and succeed.
    # When restarting, however, the initial call to `adb wait-for-device` can happen before
    # the device is ready. If so, it is reasonable to tear down and restart again.
    # In this situation, 3 server instances are created:
    # - The initial server process, which works for a bit and gets hung
    # - The first retry, which starts the DUT down the path to recovery, and
    # - The second retry, after which the device recovers and we can make progress.
    # More starts than that are not useful.
    _MAX_SERVER_RESTARTS = 3

    def __init__(
        self,
        adb_binary_path: str,
        serial_id: str,
        output_dir: Path | None = None,
        vendor_keys_path: str | None = None,
    ):
        self._adb_binary_path: str = adb_binary_path
        self._serial_id: str = serial_id
        self._output_dir: Path = output_dir or Path(tempfile.gettempdir())
        self._vendor_keys_path: str | None = vendor_keys_path

        # Use RLock to allow reentrant calls
        self._lock: threading.RLock = threading.RLock()

        # Run state variables
        self._server_is_ready: threading.Event = threading.Event()
        self._server_stop_requested: threading.Event = threading.Event()
        self._process: subprocess.Popen[str] | None = None
        self._server_port: int | None = None
        self._server_thread: threading.Thread | None = None
        self._adb_server_count: int = 0
        atexit.register(self.stop)

    def start(self) -> None:
        thread_to_start = None
        with self._lock:
            if self._server_thread and self._server_thread.is_alive():
                _LOGGER.warning(
                    "ADB server thread is still running, not starting a new one."
                )
                return

            if self._server_thread and not self._server_thread.is_alive():
                self._server_thread = None
                self._server_stop_requested.clear()

            if self._server_is_ready.is_set():
                _LOGGER.debug("Not starting ADB server, already started.")
                return

            if self._adb_server_count >= AdbServer._MAX_SERVER_RESTARTS:
                # self._adb_server_count is updated by self._run_server() and tracks the total number
                # of server instances across both crashes and calls to self.restart().
                _LOGGER.error(
                    "Exceeded maximum number of ADB server restarts. Giving up."
                )
                raise adb_errors.AdbServerError(
                    "Exceeded maximum number of ADB server restarts. Giving up."
                )

            self._server_thread = threading.Thread(
                target=self._run_server,
                name=f"adb_server_thread_{self._serial_id}",
                daemon=True,
            )
            thread_to_start = self._server_thread
            thread_to_start.start()

        # Wait for server to be ready outside the lock to avoid deadlock
        if thread_to_start:
            while not self._server_is_ready.is_set():
                if not thread_to_start.is_alive():
                    raise adb_errors.AdbServerError(
                        "ADB server thread died before setting ready flag"
                    )
                time.sleep(0.1)

    def stop(self) -> None:
        thread_to_join = None
        with self._lock:
            if self._server_thread is None:
                _LOGGER.debug("Not stopping ADB server, already stopped.")
                return

            self._server_stop_requested.set()
            try:
                process = self._process
                if process:  # If the server died, we may not have a process.
                    process.terminate()
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        _LOGGER.warning(
                            "ADB server did not terminate, killing it..."
                        )
                        process.kill()
                        process.wait()

                thread_to_join = self._server_thread
            except Exception as e:
                _LOGGER.error(
                    f"ADB server task failed unexpectedly during stop: {e}"
                )
                raise e

        # Join the thread outside the lock to avoid deadlock
        if thread_to_join:
            thread_to_join.join(timeout=5)
            with self._lock:
                if thread_to_join.is_alive():
                    _LOGGER.warning("ADB server thread did not exit after join")
                else:
                    self._server_thread = None
                    self._server_stop_requested.clear()

    def restart(self) -> None:
        # Do not use lock here to avoid holding it during stop() join and start() wait
        self.stop()
        self.start()

    def reset_restart_count(self) -> None:
        """Resets the accumulated server restart count to zero."""
        with self._lock:
            self._adb_server_count = 0

    def host(self) -> str:
        return AdbServer._SERVER_HOST_IP

    def port(self) -> int:
        assert self._server_port
        return self._server_port

    def _run_server(self) -> None:
        """Runs (and re-runs) adb server until another thread requests that it stop.

        This method manages restarting the adb server when it exits. It continues doing so
        until either it hits AdbServer._MAX_SERVER_RESTARTS successive tries, or another thread sets
        self._server_stop_requested.

        It also has one important side effect: incrementing self._adb_server_count, which tracks
        the overall number of server processes across both crashes AND calls to self.restart().

        Raises:
            RuntimeError: If the server crashes too many times in a row, this method gives up and
                          raises RuntimeError. This exception does not propagate across threads.
        """
        # Handle adb server crashes by restarting the server.
        # TODO(b/331678515): Remove auto-restart once
        # bugs like b/331678515 are solved?
        adb_server_restarts: int = 0
        while not self._server_stop_requested.is_set():
            if os.getppid() == 1:
                break
            if adb_server_restarts >= AdbServer._MAX_SERVER_RESTARTS:
                _LOGGER.error(
                    "Giving up running ADB server due to repeated crashes"
                )
                raise RuntimeError(
                    "ADB server failed to start multiple times. Giving up."
                )
            if adb_server_restarts > 0:
                _LOGGER.info("Restarting ADB server...")
                self._server_stop_requested.wait(1)

            adb_server_restarts += 1
            with self._lock:
                self._adb_server_count += 1
                server_name = f"adb_server_{self._adb_server_count}"

            try:
                self._run_one_server(server_name)
            except Exception as e:
                _LOGGER.warning(f"Failed to run ADB server: {e}")
                raise e

    def _kill_shared_server(self) -> None:
        _LOGGER.info("Killing any existing shared ADB servers.")
        result = subprocess.run(
            [self._adb_binary_path, "kill-server"],
            capture_output=True,
            text=True,
        )
        if result.stdout:
            _LOGGER.debug(f"Output (stdout): {result.stdout}")
        if result.stderr:
            _LOGGER.debug(f"Output (stderr): {result.stderr}")
        _LOGGER.info(
            "Waiting 5 seconds to give time for the ADB server to stop..."
        )
        time.sleep(5)

    def _check_for_conflicting_adb_servers(self) -> None:
        """Checks for conflicting adb server processes.

        If a conflicting adb server is found, logs an error.
        A conflicting server is defined as one that:
        1. Does not specify --one-device
        2. Specifies --one-device with our target serial ID
        """
        try:
            # pgrep -a adb lists all processes matching 'adb' along with their full command line
            result = subprocess.run(
                ["pgrep", "-a", "adb"],
                capture_output=True,
                text=True,
                check=False,
            )
        except Exception as e:
            _LOGGER.warning(
                f"Failed to run pgrep to check for ADB servers: {e}"
            )
            return

        if result.returncode != 0 and result.returncode != 1:
            _LOGGER.warning(
                f"pgrep failed with return code {result.returncode}"
            )
            return

        for line in result.stdout.splitlines():
            parts = line.split(maxsplit=1)
            if len(parts) < 2:
                continue
            pid, cmd = parts

            cmd_args = cmd.split()
            if "server" not in cmd_args and "fork-server" not in cmd_args:
                continue

            is_conflicting = False
            if "--one-device" not in cmd_args:
                is_conflicting = True
            elif self._serial_id:
                try:
                    idx = cmd_args.index("--one-device")
                    if (
                        idx + 1 < len(cmd_args)
                        and cmd_args[idx + 1] == self._serial_id
                    ):
                        is_conflicting = True
                except ValueError:
                    pass

            if is_conflicting:
                error_msg = (
                    f"Found conflicting ADB server process (PID: {pid}): {cmd}"
                )
                _LOGGER.error(error_msg)

    def _run_one_server(self, server_name: str) -> None:
        """Runs a server and waits on the process.

        This method manages self._process, a subprocess object wrapped around an isolated adb
        server. It communicates whether self._process is expected to be alive and responsive by
        setting and clearing self._server_is_ready. While self._process is running, this method
        blocks.

        Before running an isolated server, this method attempts to kill any shared adb server
        running on the host.

        Args:
            server_name: Used in log messages and in the log file name. Caller is responsible for
                         ensuring uniqueness.
        """
        assert self._serial_id
        assert self._process is None
        assert not self._server_is_ready.is_set(), "Already running"
        temp_keys_dir = None

        try:
            self._kill_shared_server()
            self._check_for_conflicting_adb_servers()

            with self._lock:
                if self._server_stop_requested.is_set():
                    _LOGGER.info(
                        "Stop requested before starting ADB server process."
                    )
                    return

                if self._server_port is None:
                    # Selecting a random port:
                    with socket.socket(
                        socket.AF_INET, socket.SOCK_STREAM
                    ) as sock:
                        sock.bind(("", 0))
                        self._server_port = sock.getsockname()[1]

                server_cmd: list[str] = [
                    self._adb_binary_path,
                    "-P",
                    str(self._server_port),
                    "--one-device",
                    self._serial_id,
                    "server",
                    "nodaemon",
                ]

                server_env = os.environ.copy()
                server_env["ADB_TRACE"] = "all"
                server_env["ADB_MDNS_OPENSCREEN"] = "0"

                if self._vendor_keys_path:
                    if self._vendor_keys_path.endswith(".tar"):
                        temp_keys_dir = tempfile.TemporaryDirectory()
                        target_dir = Path(temp_keys_dir.name).resolve()
                        with tarfile.open(self._vendor_keys_path, "r") as tar:
                            for member in tar.getmembers():
                                if member.issym() or member.islnk():
                                    raise adb_errors.AdbServerError(
                                        f"Symbolic or hard links are not allowed in key tar: {member.name}"
                                    )
                                # Prevent path traversal
                                member_path = target_dir.joinpath(
                                    member.name
                                ).resolve()
                                try:
                                    member_path.relative_to(target_dir)
                                except ValueError:
                                    raise adb_errors.AdbServerError(
                                        f"Unsafe member in tar file: {member.name}"
                                    )
                            tar.extractall(path=temp_keys_dir.name)
                        server_env["ADB_VENDOR_KEYS"] = temp_keys_dir.name
                        _LOGGER.info(
                            f"Extracted adb keys from {self._vendor_keys_path} to {temp_keys_dir.name}"
                        )
                    else:
                        server_env["ADB_VENDOR_KEYS"] = self._vendor_keys_path

                log_file_path = self._output_dir / f"{server_name}_log.txt"
                output_file = open(log_file_path, "w", encoding="utf-8")
                try:
                    _LOGGER.info(
                        f"Starting {server_name} on port {self._server_port}."
                    )
                    self._process = subprocess.Popen(
                        server_cmd,
                        env=server_env,
                        stdout=output_file,
                        stderr=subprocess.STDOUT,
                        text=True,
                    )
                finally:
                    output_file.close()

            # Poll until the port is open
            start_time = time.time()
            port_open = False
            while time.time() - start_time < 5:
                if self._server_stop_requested.is_set():
                    break
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
                    if (
                        sock.connect_ex(
                            (AdbServer._SERVER_HOST_IP, self._server_port)
                        )
                        == 0
                    ):
                        port_open = True
                        break
                time.sleep(0.1)

            if self._server_stop_requested.is_set():
                _LOGGER.info("Stop requested during port polling.")
                return

            if not port_open:
                raise adb_errors.AdbServerError(
                    f"ADB server failed to bind to port {self._server_port} within 5 seconds"
                )

            _LOGGER.info(f"Started {server_name} on port {self._server_port}.")

            self._server_is_ready.set()
            self._process.wait()

            _LOGGER.info(f"Stopped {server_name} on port {self._server_port}.")

            self._server_is_ready.clear()

            if self._server_stop_requested.is_set():
                _LOGGER.info(f"Stopped {server_name} by request.")
            else:
                _LOGGER.warning(
                    f"Stopped {server_name} unexpectedly, returncode={self._process.returncode}"
                )

            _LOGGER.info(f"{server_name} output is saved to {log_file_path}")

        finally:
            if temp_keys_dir:
                try:
                    temp_keys_dir.cleanup()
                except Exception as e:
                    _LOGGER.warning(f"Failed to cleanup temp keys dir: {e}")
            with self._lock:
                if self._process:
                    if self._process.poll() is None:
                        _LOGGER.warning(
                            "Terminating leaked ADB server process."
                        )
                        self._process.terminate()
                        try:
                            self._process.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            self._process.kill()
                            self._process.wait()
                self._process = None
                self._server_port = None
