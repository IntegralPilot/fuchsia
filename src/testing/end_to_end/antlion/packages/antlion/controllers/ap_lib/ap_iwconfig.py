#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from antlion.controllers.access_point import AccessPoint


class ApIwconfigError(Exception):
    """Error related to configuring the wireless interface via iwconfig."""


class ApIwconfig(object):
    """Class to configure wireless interface via iwconfig"""

    PROGRAM_FILE = "/usr/local/sbin/iwconfig"

    def __init__(self, ap: "AccessPoint") -> None:
        """Initialize the ApIwconfig class.

        Args:
            ap: the ap object within ACTS
        """
        self.ssh = ap.ssh

    def ap_iwconfig(
        self, interface: str, arguments: str | None = None
    ) -> subprocess.CompletedProcess[bytes]:
        """Configure the wireless interface using iwconfig.

        Returns:
            output: the output of the command, if any
        """
        return self.ssh.run(f"{self.PROGRAM_FILE} {interface} {arguments}")
