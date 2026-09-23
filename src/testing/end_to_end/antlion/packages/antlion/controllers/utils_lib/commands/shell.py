# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import re
import shlex
import signal
import time
from typing import Iterator

from libs.proc.runner import CalledProcessError, Runner


class ShellCommand(object):
    """Wraps basic commands that tend to be tied very closely to a shell.

    This class is a wrapper for running basic shell commands through
    any object that has a run command. Basic shell functionality for managing
    the system, programs, and files in wrapped within this class.

    Note: At the moment this only works with the ssh runner.
    """

    def __init__(self, runner: Runner) -> None:
        """Creates a new shell command invoker.

        Args:
            runner: The object that will run the shell commands.
        """
        self._runner = runner

    def is_alive(self, identifier: str | int) -> bool:
        """Checks to see if a program is alive.

        Checks to see if a program is alive on the shells environment. This can
        be used to check on generic programs, or a specific program using a pid.

        Args:
            identifier: Used to identify the program to check. if given an int
                then it is assumed to be a pid. If given a string then it will
                be used as a search key to compare on the running processes.
        Returns:
            True if a process was found running, otherwise False.
        """
        try:
            if isinstance(identifier, str):
                ps = self._runner.run(["ps", "aux"])
                if re.search(identifier, ps.stdout.decode("utf-8")):
                    return True
                return False
            else:
                self.signal(identifier, 0)
                return True
        except CalledProcessError:
            return False

    def get_pids(self, identifier: str) -> Iterator[int]:
        """Gets the pids of a program.

        Searches for a program with a specific name and grabs the pids for all
        programs that match.

        Args:
            identifier: A search term that identifies the program.

        Returns: An array of all pids that matched the identifier, or None
                  if no pids were found.
        """
        try:
            ps = self._runner.run(["ps", "aux"])
        except CalledProcessError as e:
            if e.returncode == 1:
                # Grep returns exit status 1 when no lines are selected. This is
                # an expected return code.
                return
            raise e

        lines = ps.stdout.decode("utf-8").splitlines()

        # The expected output of the above command is like so:
        # bob    14349  0.0  0.0  34788  5552 pts/2    Ss   Oct10   0:03 bash
        # bob    52967  0.0  0.0  34972  5152 pts/4    Ss   Oct10   0:00 bash
        # Where the format is:
        # USER    PID  ...
        for line in lines:
            if re.search(identifier, line) is None:
                continue

            pieces = line.split()
            try:
                yield int(pieces[1])
            except StopIteration:
                return

    def search_file(self, search_string: str, file_name: str) -> bool:
        """Searches through a file for a string.

        Args:
            search_string: The string or pattern to look for.
            file_name: The name of the file to search.

        Returns:
            True if the string or pattern was found, False otherwise.
        """
        try:
            self._runner.run(["grep", shlex.quote(search_string), file_name])
            return True
        except CalledProcessError:
            return False

    def read_file(self, file_name: str) -> str:
        """Reads a file through the shell.

        Args:
            file_name: The name of the file to read.

        Returns:
            A string of the files contents.
        """
        return self._runner.run(["cat", file_name]).stdout.decode("utf-8")

    def write_file(self, file_name: str, data: str) -> None:
        """Writes a block of data to a file through the shell.

        Args:
            file_name: The name of the file to write to.
            data: The string of data to write.
        """
        # Intentionally not passed through shlex.escape() to allow stdin
        # redirection to a remote file.
        self._runner.run(
            ["cat", "-", ">", file_name], stdin=data.encode("utf-8")
        )

    def touch_file(self, file_name: str) -> None:
        """Creates a file through the shell.

        Args:
            file_name: The name of the file to create.
        """
        self._runner.run(["touch", file_name])

    def delete_file(self, file_name: str) -> None:
        """Deletes a file through the shell.

        Args:
            file_name: The name of the file to delete.
        """
        try:
            self._runner.run(["rm", "-r", file_name])
        except CalledProcessError as e:
            if b"No such file or directory" in e.stderr:
                return
            raise e

    def kill(self, identifier: str | int, timeout_sec: int = 10) -> None:
        """Kills a program or group of programs through the shell.

        Kills all programs that match an identifier through the shell. This
        will send an increasing queue of kill signals to all programs
        that match the identifier until either all are dead or the timeout
        finishes.

        Programs are guaranteed to be killed after running this command.

        Args:
            identifier: A string used to identify the program.
            timeout_sec: The time to wait for all programs to die. Each signal
                will take an equal portion of this time.
        """
        if isinstance(identifier, int):
            pids = [identifier]
        else:
            pids = list(self.get_pids(identifier))

        signal_queue = [signal.SIGINT, signal.SIGTERM, signal.SIGKILL]

        signal_duration = timeout_sec / len(signal_queue)
        for sig in signal_queue:
            for pid in pids:
                try:
                    self.signal(pid, sig)
                except CalledProcessError:
                    pass

            start_time = time.time()
            while pids and time.time() - start_time < signal_duration:
                time.sleep(0.1)
                pids = [pid for pid in pids if self.is_alive(pid)]

            if not pids:
                break

    def signal(self, pid: int, sig: int) -> None:
        """Sends a specific signal to a program.

        Args:
            pid: The process id of the program to kill.
            sig: The signal to send.

        Raises:
            CalledProcessError: Raised when the signal fail to reach
                       the specified program.
        """
        self._runner.run(["kill", f"-{sig}", str(pid)])
