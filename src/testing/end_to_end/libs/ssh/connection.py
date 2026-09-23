# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import logging
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
from typing import IO

from libs.proc import job
from libs.proc.runner import (
    CalledProcessError,
    CalledProcessTransportError,
    Runner,
)
from libs.ssh import formatter
from mobly import logger


class SshConnection(Runner):
    """Provides a connection to a remote machine through ssh.

    Provides the ability to connect to a remote machine and execute a command
    on it. The connection will try to establish a persistent connection When
    a command is run. If the persistent connection fails it will attempt
    to connect normally.
    """

    @property
    def socket_path(self):
        """Returns: The os path to the main socket file."""
        if self._main_ssh_tempdir is None:
            raise AttributeError(
                "socket_path is not available yet; run setup_main_ssh() first"
            )
        return os.path.join(self._main_ssh_tempdir, "socket")

    def __init__(self, settings):
        """
        Args:
            settings: The ssh settings to use for this connection.
            formatter: The object that will handle formatting ssh command
                       for use with the background job.
        """
        self._settings = settings
        self._formatter = formatter.SshFormatter()
        self._lock = threading.Lock()
        self._main_ssh_proc = None
        self._main_ssh_tempdir: str | None = None

        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[SshConnection | {self._settings.hostname}]",
            },
        )

    def __enter__(self):
        return self

    def __exit__(self, _, __, ___):
        self.close()

    def __del__(self):
        self.close()

    def setup_main_ssh(self, timeout_sec: int = 5):
        """Sets up the main ssh connection.

        Sets up the initial main ssh connection if it has not already been
        started.

        Args:
            timeout_sec: The time to wait for the main ssh connection to
                be made.

        Raises:
            Error: When setting up the main ssh connection fails.
        """
        with self._lock:
            if self._main_ssh_proc is not None:
                socket_path = self.socket_path
                if (
                    not os.path.exists(socket_path)
                    or self._main_ssh_proc.poll() is not None
                ):
                    self.log.debug(
                        "main ssh connection to %s is down.",
                        self._settings.hostname,
                    )
                    self._cleanup_main_ssh()

            if self._main_ssh_proc is None:
                # Create a shared socket in a temp location.
                self._main_ssh_tempdir = tempfile.mkdtemp(prefix="ssh-main")

                # Setup flags and options for running the main ssh
                # -N: Do not execute a remote command.
                # ControlMaster: Spawn a main connection.
                # ControlPath: The main connection socket path.
                extra_flags: dict[str, str | int | None] = {"-N": None}
                extra_options: dict[str, str | int | bool] = {
                    "ControlMaster": True,
                    "ControlPath": self.socket_path,
                    "BatchMode": True,
                }

                # Construct the command and start it.
                main_cmd = self._formatter.format_ssh_local_command(
                    self._settings,
                    extra_flags=extra_flags,
                    extra_options=extra_options,
                )
                self.log.info("Starting main ssh connection.")
                self._main_ssh_proc = job.run_async(main_cmd)

                end_time = time.time() + timeout_sec

                while time.time() < end_time:
                    if os.path.exists(self.socket_path):
                        break
                    time.sleep(0.2)
                else:
                    self._cleanup_main_ssh()
                    raise CalledProcessTransportError(
                        "main ssh connection timed out."
                    )

    def run(
        self,
        command: str | list[str],
        stdin: bytes | None = None,
        timeout_sec: float | None = 60.0,
        log_output: bool = True,
        ignore_status: bool = False,
        attempts: int = 2,
    ) -> subprocess.CompletedProcess[bytes]:
        """Runs a remote command over ssh.

        Will ssh to a remote host and run a command. This method will
        block until the remote command is finished.

        Args:
            command: The command to execute over ssh.
            stdin: Standard input to command.
            timeout_sec: seconds to wait for command to finish.
            log_output: If true, print stdout and stderr to the debug log.
            ignore_status: True to ignore the exit code of the remote
                           subprocess.  Note that if you do ignore status codes,
                           you should handle non-zero exit codes explicitly.
            attempts: Number of attempts before giving up on command failures.

        Returns:
            Results of the ssh command.

        Raises:
            CalledProcessError: when the process exits with a non-zero status
                and ignore_status is False.
            subprocess.TimeoutExpired: When the remote command took to long to
                execute.
            CalledProcessTransportError: when the underlying transport fails
        """
        if attempts < 1:
            raise TypeError("attempts must be a positive, non-zero integer")

        try:
            self.setup_main_ssh(self._settings.connect_timeout)
        except CalledProcessTransportError:
            self.log.warning(
                "Failed to create main ssh connection, using "
                "normal ssh connection."
            )

        extra_options: dict[str, str | int | bool] = {"BatchMode": True}
        if self._main_ssh_proc:
            extra_options["ControlPath"] = self.socket_path

        if isinstance(command, list):
            full_command = " ".join(command)
        else:
            full_command = command

        terminal_command = self._formatter.format_command(
            full_command, self._settings, extra_options=extra_options
        )

        dns_retry_count = 2
        while True:
            try:
                result = job.run(
                    terminal_command,
                    stdin=stdin,
                    log_output=log_output,
                    timeout_sec=timeout_sec,
                )

                return subprocess.CompletedProcess(
                    terminal_command,
                    result.returncode,
                    result.stdout,
                    result.stderr,
                )
            except CalledProcessError as e:
                # Check for SSH errors.
                if e.returncode == 255:
                    stderr = e.stderr.decode("utf-8", errors="replace")

                    had_dns_failure = re.search(
                        r"^ssh: .*: Name or service not known",
                        stderr,
                        flags=re.MULTILINE,
                    )
                    if had_dns_failure:
                        dns_retry_count -= 1
                        if not dns_retry_count:
                            raise CalledProcessTransportError(
                                "DNS failed to find host"
                            ) from e
                        self.log.debug("Failed to connect to host, retrying...")
                        continue

                    had_timeout = re.search(
                        r"^ssh: connect to host .* port .*: "
                        r"Connection timed out\r$",
                        stderr,
                        flags=re.MULTILINE,
                    )
                    if had_timeout:
                        raise CalledProcessTransportError(
                            "Ssh timed out"
                        ) from e

                    permission_denied = "Permission denied" in stderr
                    if permission_denied:
                        raise CalledProcessTransportError(
                            "Permission denied"
                        ) from e

                    unknown_host = re.search(
                        r"ssh: Could not resolve hostname .*: "
                        r"Name or service not known",
                        stderr,
                        flags=re.MULTILINE,
                    )
                    if unknown_host:
                        raise CalledProcessTransportError("Unknown host") from e

                    # Retry unknown SSH errors.
                    self.log.error(
                        f"An unknown error has occurred. Job result: {e}"
                    )
                    ping_output = job.run(
                        ["ping", self._settings.hostname, "-c", "3", "-w", "1"],
                        ignore_status=True,
                    )
                    self.log.error(f"Ping result: {ping_output}")
                    if attempts > 1:
                        self._cleanup_main_ssh()
                        return self.run(
                            command,
                            stdin,
                            timeout_sec,
                            log_output,
                            ignore_status,
                            attempts - 1,
                        )
                    raise CalledProcessTransportError(
                        "The job failed for unknown reasons"
                    ) from e
                if ignore_status:
                    return subprocess.CompletedProcess(
                        terminal_command,
                        e.returncode,
                        e.stdout,
                        e.stderr,
                    )
                raise e

    def run_async(self, command: str) -> subprocess.CompletedProcess[bytes]:
        """Starts up a background command over ssh.

        Will ssh to a remote host and startup a command. This method will
        block until there is confirmation that the remote command has started.

        Args:
            command: The command to execute over ssh. Can be either a string
                     or a list.

        Returns:
            The result of the command to launch the background job.

        Raises:
            CalledProcessError: when the process fails to start
            subprocess.TimeoutExpired: when the timeout expires while waiting
                for a child process
            CalledProcessTransportError: when the underlying transport fails
        """
        return self.run(
            f"({command}) < /dev/null > /dev/null 2>&1 & echo -n $!"
        )

    def start(
        self,
        command: list[str],
        stdout: IO[bytes] | int = subprocess.PIPE,
        stdin: IO[bytes] | int = subprocess.PIPE,
        stderr: IO[bytes] | int = subprocess.PIPE,
    ) -> subprocess.Popen[bytes]:
        """Execute a child program in a new process."""
        extra_options: dict[str, str | int | bool] = {"BatchMode": True}
        if self._main_ssh_proc:
            extra_options["ControlPath"] = self.socket_path

        terminal_command = self._formatter.format_command(
            " ".join(command),
            self._settings,
            extra_options=extra_options,
        )
        return subprocess.Popen(
            terminal_command, stdout=stdout, stdin=stdin, stderr=stderr
        )

    def close(self) -> None:
        """Clean up open connections to remote host."""
        self._cleanup_main_ssh()

    def _cleanup_main_ssh(self) -> None:
        """
        Release all resources (process, temporary directory) used by an active
        main SSH connection.
        """
        # If a main SSH connection is running, kill it.
        if self._main_ssh_proc is not None:
            self.log.debug("Nuking main_ssh_job.")
            self._main_ssh_proc.kill()
            self._main_ssh_proc.wait()
            self._main_ssh_proc = None

        # Remove the temporary directory for the main SSH socket.
        if self._main_ssh_tempdir is not None:
            self.log.debug("Cleaning main_ssh_tempdir.")
            shutil.rmtree(self._main_ssh_tempdir)
            self._main_ssh_tempdir = None
