#!/usr/bin/env python3

# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
import shutil

from antlion.controllers import sniffer
from antlion.controllers.sniffer_lib.local import local_base


class Sniffer(local_base.SnifferLocalBase):
    """This class defines a sniffer which uses tcpdump as its back-end"""

    def __init__(self, config_path, logger, base_configs=None):
        """See base class documentation"""
        self._executable_path = None

        super().__init__(config_path, logger, base_configs=base_configs)

        self._executable_path = shutil.which("tcpdump")
        if self._executable_path is None:
            raise sniffer.SnifferError(
                "Cannot find a path to the 'tcpdump' executable"
            )

    def get_descriptor(self):
        """See base class documentation"""
        return f"local-tcpdump-{self._interface}"

    def get_subtype(self):
        """See base class documentation"""
        return "tcpdump"

    def _get_command_line(
        self, additional_args=None, duration=None, packet_count=None
    ):
        cmd = "{} -i {} -w {}".format(
            self._executable_path, self._interface, self._temp_capture_file_path
        )
        if packet_count is not None:
            cmd = f"{cmd} -c {packet_count}"
        if additional_args is not None:
            cmd = f"{cmd} {additional_args}"
        return cmd
