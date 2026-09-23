#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from __future__ import annotations

import logging
import os
import shlex
import signal
import subprocess
import sys
import time
from collections.abc import Callable
from threading import Thread
from typing import Self


class ProcessError(Exception):
    """Raised when invalid operations are run on a Process."""


class Process(object):
    """A Process object used to run various commands.

    Attributes:
        _command: The initial command to run.
        _subprocess_kwargs: The kwargs to send to Popen for more control over
                            execution.
        _process: The subprocess.Popen object currently executing a process.
        _listening_thread: The thread that is listening for the process to stop.
        _redirection_thread: The thread that is redirecting process output.
        _on_output_callback: The callback to call when output is received.
        _on_terminate_callback: The callback to call when the process terminates
                                without stop() being called first.
        _started: Whether or not start() was called.
        _stopped: Whether or not stop() was called.
    """

    def __init__(self, command: list[str] | str) -> None:
        """Creates a Process object.

        Note that this constructor does not begin the process. To start the
        process, use Process.start().
        """
        if isinstance(command, str):
            # Split command string into list
            command = shlex.split(command)
        self._command = command

        self._process: subprocess.Popen[bytes] | None = None

        self._listening_thread: Thread | None = None
        self._redirection_thread: Thread | None = None
        self._on_output_callback: Callable[[str | bytes], None] = lambda _: None
        self._binary_output: bool = False
        self._on_terminate_callback: Callable[
            [subprocess.Popen[bytes]], list[str] | str
        ] = lambda _: ""

        self._started: bool = False
        self._stopped: bool = False

    def set_on_output_callback(
        self,
        on_output_callback: Callable[[str | bytes], None],
        binary: bool = False,
    ) -> Self:
        """Sets the on_output_callback function.

        Args:
            on_output_callback: The function to be called when output is sent to
                the output. The output callback has the following signature:

                >>> def on_output_callback(output_line):
                >>>     return None

            binary: If True, read the process output as raw binary.
        Returns:
            self
        """
        self._on_output_callback = on_output_callback
        self._binary_output = binary
        return self

    def set_on_terminate_callback(
        self,
        on_terminate_callback: Callable[
            [subprocess.Popen[bytes]], list[str] | str
        ],
    ) -> Self:
        """Sets the on_self_terminate callback function.

        Args:
            on_terminate_callback: The function to be called when the process
                has terminated on its own. The callback has the following
                signature:

                >>> def on_self_terminate_callback(popen_process):
                >>>     return 'command to run' or None

                If a string is returned, the string returned will be the command
                line used to run the command again. If None is returned, the
                process will end without restarting.

        Returns:
            self
        """
        self._on_terminate_callback = on_terminate_callback
        return self

    def start(self) -> None:
        """Starts the process's execution."""
        if self._started:
            raise ProcessError("Process has already started.")
        self._started = True
        self._process = None

        self._listening_thread = Thread(target=self._exec_loop)
        self._listening_thread.start()

        time_up_at = time.time() + 1

        while self._process is None:
            if time.time() > time_up_at:
                raise OSError("Unable to open process!")

        self._stopped = False

    @staticmethod
    def _get_timeout_left(timeout, start_time) -> float:
        return max(0.1, timeout - (time.time() - start_time))

    def is_running(self) -> bool:
        """Checks that the underlying Popen process is still running

        Returns:
            True if the process is running.
        """
        return self._process is not None and self._process.poll() is None

    def _join_threads(self) -> None:
        """Waits for the threads associated with the process to terminate."""
        if self._listening_thread is not None:
            self._listening_thread.join()
            self._listening_thread = None

        if self._redirection_thread is not None:
            self._redirection_thread.join()
            self._redirection_thread = None

    def _kill_process(self) -> None:
        """Kills the underlying process/process group. Implementation is
        platform-dependent."""
        if sys.platform == "win32":
            subprocess.check_call(f"taskkill /F /T /PID {self._process.pid}")
        else:
            self.signal(signal.SIGKILL)

    def wait(self, kill_timeout: float = 60.0) -> None:
        """Waits for the process to finish execution.

        If the process has reached the kill_timeout, the process will be killed
        instead.

        Note: the on_self_terminate callback will NOT be called when calling
        this function.

        Args:
            kill_timeout: The amount of time to wait until killing the process.
        """
        if self._stopped or self._process is None:
            raise ProcessError("Process is already being stopped.")
        self._stopped = True

        try:
            self._process.wait(kill_timeout)
        except subprocess.TimeoutExpired:
            self._kill_process()
        finally:
            self._join_threads()
            self._started = False

    def signal(self, sig) -> None:
        """Sends a signal to the process.

        Args:
            sig: The signal to be sent.
        """
        if sys.platform == "win32":
            raise ProcessError("Unable to call Process.signal on windows.")
        if self._process is None:
            raise ProcessError("No process is running")

        pgid = os.getpgid(self._process.pid)
        os.killpg(pgid, sig)

    def stop(self) -> None:
        """Stops the process.

        This command is effectively equivalent to kill, but gives time to clean
        up any related work on the process, such as output redirection.

        Note: the on_self_terminate callback will NOT be called when calling
        this function.
        """
        self.wait(0)

    def _redirect_output(self) -> None:
        """Redirects the output from the command into the on_output_callback."""
        if self._process is None:
            raise ProcessError("No process is running")
        if self._process.stdout is None:
            raise ProcessError("Process stdout is not PIPE")

        while True:
            data: str | bytes
            if self._binary_output:
                data = self._process.stdout.read(1024)
            else:
                data = (
                    self._process.stdout.readline()
                    .decode("utf-8", errors="replace")
                    .rstrip()
                )

            if not data:
                return
            else:
                self._on_output_callback(data)

    def _exec_loop(self) -> None:
        """Executes Popen in a loop.

        When Popen terminates without stop() being called,
        self._on_terminate_callback() will be called. The returned value from
        _on_terminate_callback will then be used to determine if the loop should
        continue and start up the process again. See set_on_terminate_callback()
        for more information.
        """
        command = self._command
        while True:
            acts_logger = logging.getLogger()
            acts_logger.debug('Starting command "%s"', command)

            creationflags: int = 0
            if sys.platform == "win32":
                creationflags = subprocess.CREATE_NEW_PROCESS_GROUP

            self._process = subprocess.Popen(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                creationflags=creationflags,
                start_new_session=False if sys.platform == "win32" else True,
                bufsize=1,
            )
            self._redirection_thread = Thread(target=self._redirect_output)
            self._redirection_thread.start()
            self._process.wait()

            if self._stopped:
                logging.debug(
                    "The process for command %s was stopped.", command
                )
                break
            else:
                logging.debug("The process for command %s terminated.", command)
                # Wait for all output to be processed before sending
                # _on_terminate_callback()
                self._redirection_thread.join()
                logging.debug(
                    "Beginning on_terminate_callback for %s.", command
                )
                retry_value = self._on_terminate_callback(self._process)
                if retry_value:
                    if isinstance(retry_value, str):
                        retry_value = shlex.split(retry_value)
                    command = retry_value
                else:
                    break
