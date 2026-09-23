# Copyright 2026 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import logging
import os
import shlex
import subprocess
import time

from libs.proc.runner import CalledProcessError, CompletedProcess


class Result(CompletedProcess):
    """Command execution result.

    Contains information on subprocess execution after it has exited.

    Attributes:
        command: An array containing the command and all arguments that
                 was executed.
        exit_status: Integer exit code of the process.
        stdout_raw: The raw bytes output from standard out.
        stderr_raw: The raw bytes output from standard error
        duration: How long the process ran for.
        did_timeout: True if the program timed out and was killed.
    """

    def __init__(
        self,
        command: str | list[str],
        stdout: bytes,
        stderr: bytes,
        exit_status: int,
        duration: float = 0,
        did_timeout: bool = False,
        encoding: str = "utf-8",
    ) -> None:
        """
        Args:
            command: The command that was run. This will be a list containing
                     the executed command and all args.
            stdout: The raw bytes that standard output gave.
            stderr: The raw bytes that standard error gave.
            exit_status: The exit status of the command.
            duration: How long the command ran.
            did_timeout: True if the command timed out.
            encoding: The encoding standard that the program uses.
        """
        self.command = command
        self.exit_status = exit_status
        self._raw_stdout = stdout
        self._raw_stderr = stderr
        self._stdout_str: str | None = None
        self._stderr_str: str | None = None
        self._encoding = encoding
        self.duration = duration
        self.did_timeout = did_timeout

    @property
    def stdout(self) -> str:
        """String representation of standard output."""
        if not self._stdout_str:
            self._stdout_str = self._raw_stdout.decode(
                encoding=self._encoding, errors="replace"
            )
            self._stdout_str = self._stdout_str.strip()
        return self._stdout_str

    @property
    def stderr(self) -> str:
        """String representation of standard error."""
        if not self._stderr_str:
            self._stderr_str = self._raw_stderr.decode(
                encoding=self._encoding, errors="replace"
            )
            self._stderr_str = self._stderr_str.strip()
        return self._stderr_str

    @property
    def returncode(self) -> int:
        return self.exit_status

    def __repr__(self) -> str:
        if self.did_timeout:
            prefix = f"Command timed out"
        else:
            prefix = f"Command exited with {self.exit_status}"

        command = (
            " ".join(self.command)
            if isinstance(self.command, list)
            else self.command
        )

        return (
            f"{prefix} after {self.duration}s: {command}\n"
            f"stdout: {self._raw_stdout.decode('utf-8', errors='replace')}\n"
            f"stderr: {self._raw_stderr.decode('utf-8', errors='replace')}"
        )


def run(
    command: str | list[str],
    stdin: bytes | None = None,
    timeout_sec: float | None = 60,
    log_output: bool = True,
    ignore_status: bool = False,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[bytes]:
    """Execute a command in a subprocess and return its output.

    Commands can be either shell commands (given as strings) or the
    path and arguments to an executable (given as a list).  This function
    will block until the subprocess finishes or times out.

    Args:
        command: The command to execute.
        timeout_sec: number seconds to wait for command to finish.
        log_output: If true, print stdout and stderr to the debug log.
        ignore_status: True to ignore the exit code of the remote
                       subprocess.  Note that if you do ignore status codes,
                       you should handle non-zero exit codes explicitly.
        env: environment variables to setup on the remote host.

    Returns:
        Result of the ssh command.

    Raises:
        CalledProcessError: when the process exits with a non-zero status
            and ignore_status is False.
        subprocess.TimeoutExpired: When the remote command took to long to
            execute.
    """
    start = time.perf_counter()
    proc = subprocess.Popen(
        command,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        stdin=subprocess.PIPE,
        shell=not isinstance(command, list),
    )
    # Wait on the process terminating
    timed_out = False
    stdout = bytes()
    stderr = bytes()
    try:
        (stdout, stderr) = proc.communicate(stdin, timeout_sec)
    except subprocess.TimeoutExpired:
        timed_out = True
        proc.kill()
        proc.wait()

    elapsed = time.perf_counter() - start
    exit_code = proc.poll()
    if log_output:
        logging.debug(
            "Command %s exited with %d after %.2fs\nstdout: %s\nstderr: %s",
            shlex.join(command),
            exit_code,
            elapsed,
            stdout.decode("utf-8", errors="replace"),
            stderr.decode("utf-8", errors="replace"),
        )
    else:
        logging.debug(
            "Command %s exited with %d after %.2fs",
            shlex.join(command),
            exit_code,
            elapsed,
        )

    if timed_out:
        raise subprocess.TimeoutExpired(command, elapsed, stdout, stderr)

    if not ignore_status and exit_code != 0:
        raise CalledProcessError(proc.returncode, command, stdout, stderr)

    return subprocess.CompletedProcess(command, proc.returncode, stdout, stderr)


def run_async(
    command: str | list[str], env: dict[str, str] | None = None
) -> subprocess.Popen[bytes]:
    """Execute a command in a subproccess asynchronously.

    It is the callers responsibility to kill/wait on the resulting
    subprocess.Popen object.

    Commands can be either shell commands (given as strings) or the
    path and arguments to an executable (given as a list).  This function
    will not block.

    Args:
        command: The command to execute. Can be either a string or a list.
        env: dict environment variables to setup on the remote host.

    Returns:
        A subprocess.Popen object representing the created subprocess.

    """
    proc = subprocess.Popen(
        command,
        env=env,
        preexec_fn=os.setpgrp,
        shell=not isinstance(command, list),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    logging.debug("command %s started with pid %s", command, proc.pid)
    return proc
