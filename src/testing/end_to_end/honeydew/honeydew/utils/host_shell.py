# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility module for running commands on host."""

import logging
import subprocess

from honeydew import errors
from honeydew.typing import custom_types
from honeydew.utils import decorators

_LOGGER: logging.Logger = logging.getLogger(__name__)


@decorators.liveness_check
def run(
    cmd: list[str],
    capture_output: bool = True,
    capture_error_in_output: bool = False,
    log_output: bool = True,
    timeout: float | None = None,
) -> str | None:
    """Runs the command on host shell and returns the output.

    Args:
        cmd: command to run on the host
        capture_output: When True, the stdout/stderr from the command will be
            captured and returned. When False, the output of the command
            will be streamed to stdout/stderr accordingly and it won't be
            returned. Defaults to True.
        capture_error_in_output: When True, stdout/stderr from the command will
            be captured and returned. In addition, stderr will be captured in
            stdout.
            Default value is False. Most commands does not need it. This was
            added as a special case to read `fastboot` command outputs, where
            output command is coming in stderr.
        log_output: When True, logs the output in DEBUG level. Callers
            may set this to False when expecting particularly large
            or spammy output.
        timeout: maximum amount of time to wait for the command to finish.
            Default value is None which means no time limit.

    Returns:
        Command output, if capture_output is True. Otherwise, returns None.

    Raises:
        HoneydewTimeoutError: In case of command execution results in a timeout.
        HostCmdError: In case of command execution results in a failure.
    """
    stdout: int | None = None
    stderr: int | None = None

    if capture_output:
        stdout = subprocess.PIPE
        stderr = subprocess.PIPE

    if capture_error_in_output:
        stdout = subprocess.PIPE
        stderr = subprocess.STDOUT

    message: str = f"Running the command: '{cmd}'"

    if timeout:
        message += f", with timeout={timeout}"

    if stdout == subprocess.PIPE:
        message += ", with stdout=subprocess.PIPE"
    elif stdout == subprocess.STDOUT:
        message += ", with stdout=subprocess.STDOUT"

    if stderr == subprocess.PIPE:
        message += ", with stderr=subprocess.PIPE"
    elif stderr == subprocess.STDOUT:
        message += ", with stderr=subprocess.STDOUT"

    _LOGGER.debug(message)

    try:
        proc: subprocess.CompletedProcess[str] = subprocess.run(
            cmd,
            stdout=stdout,  # whether or not to capture and where to capture stdout
            stderr=stderr,  # whether or not to capture and where to capture stderr
            check=True,  # to raise exception if cmd fails
            text=True,  # to decode the output to str
            timeout=timeout,  # default is None which means no timeout
        )
        output: str | None = proc.stdout
        if isinstance(output, str):
            output = output.strip()
        message = f"Finished executing: '{cmd}'"
        if log_output:
            message += f". Output returned: '{output}'"
        _LOGGER.debug(message)
        return output
    except subprocess.CalledProcessError as err:
        message = f"Command '{cmd}' failed. returncode = {err.returncode}"
        if err.stdout:
            message += f", stdout = {err.stdout}"
        if err.stderr:
            message += f", stderr = {err.stderr}."
        raise errors.HostCmdError(message) from err
    except subprocess.TimeoutExpired as err:
        message = f"Command : '{cmd}' timed out after {timeout}sec"
        raise errors.HoneydewTimeoutError(message) from err


def popen(  # type: ignore[no-untyped-def]
    cmd: list[str],
    text: bool = True,  # to decode the output to str
    **kwargs,  # ignore no-untyped-def because of missing type annotation for kwargs
) -> subprocess.Popen[custom_types.AnyString]:
    """Starts a new process to run the cmd and returns the corresponding
    process.

    Intended for executing daemons or processing streamed output. Given
    the raw nature of this API, it is up to callers to detect and handle
    potential errors, and make sure to close this process eventually
    (e.g. with `popen.terminate` method). Otherwise, use the simpler `run`
    method instead.

    Args:
        cmd: command to run on the host
        text: Set it to True for decoding output to str. Otherwise, False.
        kwargs: Forwarded as-is to subprocess.Popen.

    Returns:
        Child process associated with the cmd.
        If text=True, subprocess.Popen[str] will be returned.
        Otherwise, subprocess.Popen[bytes] will be returned.
    """
    _LOGGER.debug(
        "Starting a new process to run the command: '%s' with text: '%s', "
        "kwargs: '%s'",
        cmd,
        text,
        kwargs,
    )

    return subprocess.Popen(
        cmd,
        text=text,
        **kwargs,
    )
