#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import logging
import os
import shlex
import shutil
import signal
import subprocess
import time
from dataclasses import dataclass
from typing import IO, Mapping

from antlion.net import wait_for_port
from antlion.types import Json
from antlion.validation import MapValidator
from libs.proc.runner import (
    CalledProcessError,
    CalledProcessTransportError,
    Runner,
)
from mobly import logger, signals

DEFAULT_SSH_PORT: int = 22
DEFAULT_SSH_TIMEOUT_SEC: float = 60.0
DEFAULT_SSH_CONNECT_TIMEOUT_SEC: int = 90
DEFAULT_SSH_SERVER_ALIVE_INTERVAL: int = 30
# The default package repository for all components.


class SSHResult:
    """Result of an SSH command."""

    def __init__(
        self,
        process: (
            subprocess.CompletedProcess[bytes]
            | subprocess.CompletedProcess[str]
            | CalledProcessError
        ),
    ) -> None:
        if isinstance(process.stdout, bytes):
            self._stdout_bytes = process.stdout
        elif isinstance(process.stdout, str):
            self._stdout = process.stdout
        else:
            raise TypeError(
                "Expected process.stdout to be either bytes or str, "
                f"got {type(process.stdout)}"
            )

        if isinstance(process.stderr, bytes):
            self._stderr_bytes = process.stderr
        elif isinstance(process.stderr, str):
            self._stderr = process.stderr
        else:
            raise TypeError(
                "Expected process.stderr to be either bytes or str, "
                f"got {type(process.stderr)}"
            )

        self._exit_status = process.returncode

    def __str__(self) -> str:
        if self.exit_status == 0:
            return self.stdout
        return f'status {self.exit_status}, stdout: "{self.stdout}", stderr: "{self.stderr}"'

    @property
    def stdout(self) -> str:
        if not hasattr(self, "_stdout"):
            self._stdout = self._stdout_bytes.decode("utf-8", errors="replace")
        return self._stdout

    @property
    def stdout_bytes(self) -> bytes:
        if not hasattr(self, "_stdout_bytes"):
            self._stdout_bytes = self._stdout.encode()
        return self._stdout_bytes

    @property
    def stderr(self) -> str:
        if not hasattr(self, "_stderr"):
            self._stderr = self._stderr_bytes.decode("utf-8", errors="replace")
        return self._stderr

    @property
    def exit_status(self) -> int:
        return self._exit_status


class SSHError(signals.TestError):
    """A SSH command returned with a non-zero status code."""

    def __init__(
        self, command: list[str], result: CalledProcessError, elapsed_sec: float
    ):
        if result.returncode < 0:
            try:
                reason = f"died with {signal.Signals(-result.returncode)}"
            except ValueError:
                reason = f"died with unknown signal {-result.returncode}"
        else:
            reason = f"unexpectedly returned {result.returncode}"

        super().__init__(
            f'SSH command "{" ".join(command)}" {reason} after {elapsed_sec:.2f}s\n'
            f'stderr: {result.stderr.decode("utf-8", errors="replace")}\n'
            f'stdout: {result.stdout.decode("utf-8", errors="replace")}\n'
        )
        self.result = result


@dataclass
class SSHConfig:
    """SSH client config."""

    # SSH flags. See ssh(1) for full details.
    user: str
    host_name: str
    identity_file: str

    ssh_binary: str = "ssh"
    config_file: str = "/dev/null"
    port: int = 22

    #
    # SSH options. See ssh_config(5) for full details.
    #
    connect_timeout: int = DEFAULT_SSH_CONNECT_TIMEOUT_SEC
    server_alive_interval: int = DEFAULT_SSH_SERVER_ALIVE_INTERVAL
    strict_host_key_checking: bool = False
    user_known_hosts_file: str = "/dev/null"
    log_level: str = "ERROR"

    # Force allocation of a pseudo-tty. This can be used to execute arbitrary
    # screen-based programs on a remote machine, which can be very useful, e.g.
    # when implementing menu services.
    force_tty: bool = False

    def full_command(self, command: list[str]) -> list[str]:
        """Generate the complete command to execute command over SSH.

        Args:
            command: The command to run over SSH
            force_tty: Force pseudo-terminal allocation. This can be used to
                execute arbitrary screen-based programs on a remote machine,
                which can be very useful, e.g. when implementing menu services.

        Returns:
            Arguments composing the complete call to SSH.
        """
        return [
            self.ssh_binary,
            # SSH flags
            "-i",
            self.identity_file,
            "-F",
            self.config_file,
            "-p",
            str(self.port),
            # SSH configuration options
            "-o",
            f"ConnectTimeout={self.connect_timeout}",
            "-o",
            f"ServerAliveInterval={self.server_alive_interval}",
            "-o",
            f'StrictHostKeyChecking={"yes" if self.strict_host_key_checking else "no"}',
            "-o",
            f"UserKnownHostsFile={self.user_known_hosts_file}",
            "-o",
            f"LogLevel={self.log_level}",
            "-o",
            f'RequestTTY={"force" if self.force_tty else "auto"}',
            f"{self.user}@{self.host_name}",
        ] + command

    @staticmethod
    def from_config(config: Mapping[str, Json]) -> SSHConfig:
        c = MapValidator(config)
        ssh_binary_path = c.get(str, "ssh_binary_path", None)
        if ssh_binary_path is None:
            found_path = shutil.which("ssh")
            if not isinstance(found_path, str):
                raise ValueError("Failed to find ssh in $PATH")
            ssh_binary_path = found_path

        return SSHConfig(
            user=c.get(str, "user"),
            host_name=c.get(str, "host"),
            identity_file=c.get(str, "identity_file"),
            ssh_binary=ssh_binary_path,
            config_file=c.get(str, "ssh_config", "/dev/null"),
            port=c.get(int, "port", 22),
            connect_timeout=c.get(int, "connect_timeout", 30),
        )


class SSHProvider(Runner):
    """Device-specific provider for SSH clients."""

    def __init__(self, config: SSHConfig) -> None:
        """
        Args:
            config: SSH client config
        """
        logger_tag = f"ssh | {config.host_name}"
        if config.port != DEFAULT_SSH_PORT:
            logger_tag += f":{config.port}"

        # Escape IPv6 interface identifier if present.
        logger_tag = logger_tag.replace("%", "%%")

        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[{logger_tag}]",
            },
        )

        self.config = config

        try:
            self.wait_until_reachable()
            self.log.info("sshd is reachable")
        except Exception as e:
            raise TimeoutError("sshd is unreachable") from e

    def wait_until_reachable(self) -> None:
        """Wait for the device to become reachable via SSH.

        Raises:
            TimeoutError: connect_timeout has expired without a successful SSH
                connection to the device
            CalledProcessTransportError: SSH is available on the device but
                connect_timeout has expired and SSH fails to run
            subprocess.TimeoutExpired: when the timeout expires while waiting
                for a child process
        """
        timeout_sec = self.config.connect_timeout
        timeout = time.time() + timeout_sec
        wait_for_port(
            self.config.host_name, self.config.port, timeout_sec=timeout_sec
        )

        while True:
            try:
                self._run(
                    ["echo"],
                    stdin=None,
                    timeout_sec=timeout_sec,
                    log_output=True,
                )
                return
            except CalledProcessTransportError as e:
                # Repeat if necessary; _run() can exit prematurely by receiving
                # SSH transport errors. These errors can be caused by sshd not
                # being fully initialized yet.
                if time.time() < timeout:
                    continue
                else:
                    raise e

    def wait_until_unreachable(
        self,
        interval_sec: int = 1,
        timeout_sec: int = DEFAULT_SSH_CONNECT_TIMEOUT_SEC,
    ) -> None:
        """Wait for the device to become unreachable via SSH.

        Args:
            interval_sec: Seconds to wait between unreachability attempts
            timeout_sec: Seconds to wait until raising TimeoutError

        Raises:
            TimeoutError: when timeout_sec has expired without an unsuccessful
                SSH connection to the device
        """
        timeout = time.time() + timeout_sec

        while True:
            try:
                wait_for_port(
                    self.config.host_name,
                    self.config.port,
                    timeout_sec=interval_sec,
                )
            except TimeoutError:
                return

            if time.time() < timeout:
                raise TimeoutError(
                    f"Connection to {self.config.host_name} is still reachable "
                    f"after {timeout_sec}s"
                )

    def run(
        self,
        command: str | list[str],
        stdin: bytes | None = None,
        timeout_sec: float | None = DEFAULT_SSH_TIMEOUT_SEC,
        log_output: bool = True,
        connect_retries: int = 3,
    ) -> subprocess.CompletedProcess[bytes]:
        """Run a command on the device then exit.

        Args:
            command: String to send to the device.
            stdin: Standard input to command.
            timeout_sec: Seconds to wait for the command to complete.
            connect_retries: Amount of times to retry connect on fail.

        Raises:
            CalledProcessError: when the process exits with a non-zero status
            subprocess.TimeoutExpired: when the timeout expires while waiting
                for a child process
            CalledProcessTransportError: when the underlying transport fails

        Returns:
            SSHResults from the executed command.
        """
        if isinstance(command, str):
            s = shlex.shlex(command, posix=True, punctuation_chars=True)
            s.whitespace_split = True
            command = list(s)
        return self._run_with_retry(
            command, stdin, timeout_sec, log_output, connect_retries
        )

    def _run_with_retry(
        self,
        command: list[str],
        stdin: bytes | None,
        timeout_sec: float | None,
        log_output: bool,
        connect_retries: int,
    ) -> subprocess.CompletedProcess[bytes]:
        err: Exception = ValueError("connect_retries cannot be 0")
        for _ in range(0, connect_retries):
            try:
                return self._run(command, stdin, timeout_sec, log_output)
            except CalledProcessTransportError as e:
                err = e
                self.log.warning("Connect failed: %s", e)
        raise err

    def _run(
        self,
        command: list[str],
        stdin: bytes | None,
        timeout_sec: float | None,
        log_output: bool,
    ) -> subprocess.CompletedProcess[bytes]:
        start = time.perf_counter()
        with self.start(command) as process:
            try:
                stdout, stderr = process.communicate(stdin, timeout_sec)
            except subprocess.TimeoutExpired as e:
                process.kill()
                process.wait()
                raise e
            except:  # Including KeyboardInterrupt, communicate handled that.
                process.kill()
                # We don't call process.wait() as Popen.__exit__ does that for
                # us.
                raise

            elapsed = time.perf_counter() - start
            exit_code = process.poll()

            if log_output:
                self.log.debug(
                    "Command %s exited with %d after %.2fs\nstdout: %s\nstderr: %s",
                    " ".join(command),
                    exit_code,
                    elapsed,
                    stdout.decode("utf-8", errors="replace"),
                    stderr.decode("utf-8", errors="replace"),
                )
            else:
                self.log.debug(
                    "Command %s exited with %d after %.2fs",
                    " ".join(command),
                    exit_code,
                    elapsed,
                )

            if exit_code is None:
                raise ValueError(
                    f'Expected process to be terminated: "{" ".join(command)}"'
                )

            if exit_code:
                err = CalledProcessError(
                    exit_code, process.args, output=stdout, stderr=stderr
                )

                if err.returncode == 255:
                    reason = stderr.decode("utf-8", errors="replace")
                    if (
                        "Name or service not known" in reason
                        or "Host does not exist" in reason
                    ):
                        raise CalledProcessTransportError(
                            f"Hostname {self.config.host_name} cannot be resolved to an address"
                        ) from err
                    if "Connection timed out" in reason:
                        raise CalledProcessTransportError(
                            f"Failed to establish a connection to {self.config.host_name} within {timeout_sec}s"
                        ) from err
                    if "Connection refused" in reason:
                        raise CalledProcessTransportError(
                            f"Connection refused by {self.config.host_name}"
                        ) from err

                raise err

        return subprocess.CompletedProcess(
            process.args, exit_code, stdout, stderr
        )

    def run_async(self, command: str) -> subprocess.CompletedProcess[bytes]:
        s = shlex.shlex(command, posix=True, punctuation_chars=True)
        s.whitespace_split = True
        command_split = list(s)

        process = self.start(command_split)
        return subprocess.CompletedProcess(
            self.config.full_command(command_split),
            returncode=0,
            stdout=str(process.pid).encode("utf-8"),
            stderr=None,
        )

    def start(
        self,
        command: list[str],
        stdout: IO[bytes] | int = subprocess.PIPE,
        stdin: IO[bytes] | int = subprocess.PIPE,
        stderr: IO[bytes] | int = subprocess.PIPE,
    ) -> subprocess.Popen[bytes]:
        full_command = self.config.full_command(command)
        self.log.debug(
            f"Starting: {' '.join(command)}\nFull command: {' '.join(full_command)}"
        )
        return subprocess.Popen(
            full_command,
            stdin=stdin,
            stdout=stdout if stdout else subprocess.PIPE,
            stderr=stderr,
            preexec_fn=os.setpgrp,
        )
