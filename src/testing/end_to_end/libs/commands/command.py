# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
from typing import IO, Protocol, TypeVar

from libs.proc.runner import CalledProcessError, Runner
from mobly import signals


class Command(Protocol):
    """A runnable binary."""

    def binary(self) -> str:
        """Return the binary used for this command."""
        ...

    def available(self) -> bool:
        """Return true if this command is available to run."""
        ...


_C = TypeVar("_C", bound=Command)


def require(command: _C) -> _C:
    """Require a command to be available."""
    if command.available():
        return command
    raise signals.TestAbortClass(
        f"Required command not found: {command.binary()}"
    )


def optional(command: _C) -> _C | None:
    """Optionally require a command to be available."""
    if command.available():
        return command
    return None


class LinuxCommand(Command):
    """A command running on a Linux machine."""

    def __init__(self, runner: Runner, binary: str) -> None:
        self._runner = runner
        self._binary = binary
        self._can_sudo = self._available("sudo")

    def binary(self) -> str:
        """Return the binary used for this command."""
        return self._binary

    def available(self) -> bool:
        """Return true if this command is available to run."""
        return self._available(self._binary)

    def _available(self, binary: str) -> bool:
        """Check if binary is available to run."""
        try:
            self._runner.run(["command", "-v", binary])
        except CalledProcessError:
            return False
        return True

    def _run(
        self,
        args: list[str],
        sudo: bool = False,
        timeout_sec: float | None = None,
        log_output: bool = True,
    ) -> subprocess.CompletedProcess[bytes]:
        """Run the command without having to specify the binary.

        Args:
            args: List of arguments to pass to the binary
            sudo: Use sudo to execute the binary, if available
            timeout_sec: Seconds to wait for command to finish
            log_output: If true, print stdout and stderr to the debug log.
        """
        if sudo and self._can_sudo:
            cmd = ["sudo", self._binary]
        else:
            cmd = [self._binary]
        return self._runner.run(
            cmd + args, timeout_sec=timeout_sec, log_output=log_output
        )

    def _start(
        self,
        args: list[str],
        sudo: bool = False,
        stdout: IO[bytes] | int = subprocess.PIPE,
        stderr: IO[bytes] | int = subprocess.PIPE,
    ) -> subprocess.Popen[bytes]:
        """Start the command without having to specify the binary.

        Args:
            args: List of arguments to pass to the binary
            sudo: Use sudo to execute the binary, if available
        """
        if sudo and self._can_sudo:
            cmd = ["sudo", self._binary]
        else:
            cmd = [self._binary]
        return self._runner.start(cmd + args, stdout, stderr=stderr)
