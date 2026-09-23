# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import subprocess
from os import PathLike
from typing import IO, Protocol, Sequence, TypeAlias

from mobly import logger, signals

StrOrBytesPath: TypeAlias = str | bytes | PathLike[str] | PathLike[bytes]
_CMD: TypeAlias = StrOrBytesPath | Sequence[StrOrBytesPath]


class Runner(Protocol):
    """A command runner."""

    log: logger.PrefixLoggerAdapter

    def run(
        self,
        command: str | list[str],
        stdin: bytes | None = None,
        timeout_sec: float | None = None,
        log_output: bool = True,
    ) -> subprocess.CompletedProcess[bytes]:
        """Run command with arguments.

        Args:
            command: Command to execute
            stdin: Standard input to command.
            timeout_sec: Seconds to wait for command to finish
            log_output: If true, print stdout and stderr to the debug log.

        Returns:
            Result of the completed command.

        Raises:
            CalledProcessError: when the process exits with a non-zero status
            subprocess.TimeoutExpired: when the timeout expires while waiting
                for a child process
            CalledProcessTransportError: when the underlying transport fails
        """
        ...

    def run_async(self, command: str) -> subprocess.CompletedProcess[bytes]:
        """Run command asynchronously.

        Args:
            command: Command to execute

        Returns:
            Results of the dispatched command.

        Raises:
            CalledProcessError: when the process fails to start
            subprocess.TimeoutExpired: when the timeout expires while waiting
                for a child process
            CalledProcessTransportError: when the underlying transport fails
        """
        ...

    def start(
        self,
        command: list[str],
        stdout: IO[bytes] | int = subprocess.PIPE,
        stdin: IO[bytes] | int = subprocess.PIPE,
        stderr: IO[bytes] | int = subprocess.PIPE,
    ) -> subprocess.Popen[bytes]:
        """Execute a child program in a new process."""
        ...


class CompletedProcess(Protocol):
    @property
    def returncode(self) -> int:
        """Exit status."""
        ...

    @property
    def stdout(self) -> str:
        """Output stream."""
        ...

    @property
    def stderr(self) -> str:
        """Error output stream."""
        ...


class CalledProcessError(subprocess.CalledProcessError):
    """Wrapper over subprocess.CalledProcessError to guarantee stdout and stderr
    are bytes and not None."""

    returncode: int
    cmd: _CMD
    output: bytes

    stdout: bytes
    stderr: bytes

    def __init__(
        self: CalledProcessError,
        returncode: int,
        cmd: _CMD,
        output: str | bytes | None = None,
        stderr: str | bytes | None = None,
    ) -> None:
        # For usability, guaranteed stdout and stderr are bytes and not None.
        if isinstance(output, str):
            output = output.encode("utf-8")
        if isinstance(stderr, str):
            stderr = stderr.encode("utf-8")
        if output is None:
            output = bytes()
        if stderr is None:
            stderr = bytes()

        super().__init__(returncode, cmd, output, stderr)

    def __str__(self) -> str:
        out = super().__str__()
        out += f'\nstderr: {self.stderr.decode("utf-8", errors="replace")}'
        out += f'\nstdout: {self.stdout.decode("utf-8", errors="replace")}'
        return out


class CalledProcessTransportError(signals.TestError):
    """Error in process's underlying transport."""
