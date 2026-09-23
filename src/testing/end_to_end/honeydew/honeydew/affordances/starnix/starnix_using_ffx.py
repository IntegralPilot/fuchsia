# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Starnix affordance implementation using FFX. """

import logging
import os
import pty
import re
import subprocess

from honeydew import errors
from honeydew.affordances.starnix import errors as starnix_errors
from honeydew.affordances.starnix import starnix
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.utils import decorators

_MAX_READ_SIZE: int = 1024

_LOGGER: logging.Logger = logging.getLogger(__name__)
_STARNIX_KERNEL_PREFIX = "core/starnix_runner/kernels:"


class _StarnixCmds:
    """Class to hold Starnix commands."""

    PREFIX: list[str] = [
        "starnix",
        "console",
        "/bin/sh",
        "-c",
    ]


class _RegExPatterns:
    """Class to hold Regular Expression patterns."""

    STARNIX_CMD_SUCCESS: re.Pattern[str] = re.compile(r"(exit code: 0)")

    STARNIX_NOT_SUPPORTED: re.Pattern[str] = re.compile(
        r"Unable to find Starnix container in the session"
    )


class StarnixUsingFfx(starnix.Starnix):
    """Async Starnix affordance implementation using FFX.

    Args:
        device_name: Device name returned by `ffx target list`.
        ffx: interfaces.transports.FFX implementation.

    Raises:
        errors.NotSupportedError: If Fuchsia device does not support Starnix.
    """

    def __init__(self, device_name: str, ffx: ffx_transport.FFX) -> None:
        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx

        self.verify_supported()

    # List all the public methods
    def verify_supported(self) -> None:
        """Verifies that the starnix affordance is supported by the Fuchsia device.

        This method should be called in `__init__()` so that if this affordance was called on a
        Fuchsia device that does not support it, it will raise NotSupportedError.

        Raises:
            NotSupportedError: If affordance is not supported.
            StarnixError: In case of other starnix command failure.
        """
        _LOGGER.debug(
            "Checking if %s supports %s affordance...",
            self._device_name,
            self.__class__.__name__,
        )

        try:
            component_list = self._ffx.run(["component", "list"])
            components = component_list.splitlines()
            if not any(
                component
                for component in components
                if component.startswith(_STARNIX_KERNEL_PREFIX)
            ):
                raise errors.NotSupportedError(
                    f"{self._device_name} does not support Starnix"
                )

        except ffx_errors.FfxCommandError as err:
            raise starnix_errors.StarnixError(err)

        _LOGGER.debug(
            "%s supports %s affordance...",
            self._device_name,
            self.__class__.__name__,
        )

    @decorators.liveness_check
    async def run_console_shell_cmd(self, cmd: list[str]) -> str:
        """Run a starnix console command and return its output.

        Args:
            cmd: cmd that need to be run excluding `starnix /bin/sh -c`.

        Returns:
            Output of `ffx -t {target} starnix /bin/sh -c {cmd}`.

        Raises:
            errors.StarnixError: In case of starnix command failure.
            errors.NotSupportedError: If Fuchsia device does not support Starnix.
        """

        # starnix console requires the process to run in tty:
        host_fd: int
        child_fd: int
        host_fd, child_fd = pty.openpty()

        starnix_cmd: list[str] = _StarnixCmds.PREFIX + cmd
        starnix_cmd_str: str = " ".join(starnix_cmd)
        process: subprocess.Popen[str] = self._ffx.popen(
            cmd=starnix_cmd,
            stdin=child_fd,
            stdout=child_fd,
            stderr=child_fd,
        )
        process.wait()

        # Note: This call may sometime return less chars than _MAX_READ_SIZE
        # even when command output contains more chars. This happened with
        # `getprop` command output but not with suspend-resume related
        # operations. So consider exploring better ways to read command output
        # such that this method can be used with other starnix console commands
        output: str = os.read(host_fd, _MAX_READ_SIZE).decode("utf-8")

        _LOGGER.debug(
            "Starnix console cmd `%s` completed. returncode=%s, output:\n%s",
            starnix_cmd_str,
            process.returncode,
            output,
        )

        if _RegExPatterns.STARNIX_CMD_SUCCESS.search(output):
            return output
        elif _RegExPatterns.STARNIX_NOT_SUPPORTED.search(output):
            board: str | None = None
            product: str | None = None
            try:
                board = self._ffx.get_target_board()
                product = self._ffx.get_target_product()
            except errors.HoneydewError:
                pass
            error_msg: str
            if board and product:
                error_msg = (
                    f"{self._device_name} running {product}.{board} does not "
                    f"support Starnix"
                )
            else:
                error_msg = f"{self._device_name} does not support Starnix"
            raise errors.NotSupportedError(error_msg)
        else:
            raise starnix_errors.StarnixError(
                f"Starnix console cmd `{starnix_cmd_str}` failed. (See debug "
                "logs for command output)"
            )
