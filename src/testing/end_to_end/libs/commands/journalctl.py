# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import shlex
from datetime import datetime

from libs.commands import pgrep
from libs.commands.command import LinuxCommand
from libs.proc.runner import Runner

# Timestamp format accepted by systemd.
# See https://man7.org/linux/man-pages/man7/systemd.time.7.html#PARSING_TIMESTAMPS
SYSTEMD_TIMESTAMP_FORMAT = "%Y-%m-%d %H:%M:%S UTC"

# Wait a maximum of 5 minutes for journalctl to output all systemd journal logs
# since boot.
JOURNALCTL_TIMEOUT_SEC = 60 * 5


class LinuxJournalctlCommand(LinuxCommand):
    """Print log entries from the systemd journal.

    Only supported on Linux distributions using systemd.
    """

    def __init__(self, runner: Runner, binary: str = "journalctl") -> None:
        super().__init__(runner, binary)
        self._pgrep = pgrep.LinuxPgrepCommand(runner)
        self._last_ran: datetime | None = None
        self._logs_before_reset: str | None = None

    def available(self) -> bool:
        if not super().available():
            return False
        if not self._pgrep.available():
            return False
        return self._pgrep.find("systemd-journal") is not None

    def logs(self) -> str:
        """Return log entries since the last run or current boot, in that order."""
        if self._last_ran:
            args = [
                "--since",
                shlex.quote(self._last_ran.strftime(SYSTEMD_TIMESTAMP_FORMAT)),
            ]
        else:
            args = ["--boot"]

        self._last_ran = datetime.utcnow()

        self._runner.log.debug("Running journalctl")
        logs = self._run(
            args,
            sudo=True,
            log_output=False,
            timeout_sec=JOURNALCTL_TIMEOUT_SEC,
        ).stdout.decode("utf-8")

        if self._logs_before_reset:
            return f"{self._logs_before_reset}\n{logs}"
        return logs

    def set_runner(self, runner: Runner) -> None:
        """Set a new runner.

        Use when underlying connection to the device refreshes.
        """
        self._runner = runner

    def save_and_reset(self) -> None:
        """Save logs and reset the last known run time.

        Run before every reboot!
        """
        self._logs_before_reset = self.logs()
        self._last_ran = None
