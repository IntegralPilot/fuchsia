# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import datetime

from libs.commands.command import LinuxCommand
from libs.proc.runner import Runner


class LinuxDateCommand(LinuxCommand):
    """Look through current running processes."""

    def __init__(self, runner: Runner, binary: str = "date") -> None:
        super().__init__(runner, binary)

    def sync(self) -> None:
        """Synchronize system time.

        Allows for better synchronization between antlion host logs and device
        logs. Useful for when the device does not have an internet connection.
        """
        now = datetime.datetime.now().astimezone().isoformat()
        self._run(["-s", now], sudo=True)
