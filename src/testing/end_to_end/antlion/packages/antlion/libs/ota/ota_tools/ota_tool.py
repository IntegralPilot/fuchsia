#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"


class OtaTool(object):
    """A Wrapper for an OTA Update command or tool.

    Each OtaTool acts as a facade to the underlying command or tool used to
    update the device.
    """

    def __init__(self, command):
        """Creates an OTA Update tool with the given properties.

        Args:
            command: A string that is used as the command line tool
        """
        self.command = command

    def update(self, ota_runner):
        """Begins the OTA Update. Returns after the update has installed.

        Args:
            ota_runner: The OTA Runner that handles the device information.
        """
        raise NotImplementedError()

    def cleanup(self, ota_runner):
        """A cleanup method for the OTA Tool to run after the update completes.

        Args:
            ota_runner: The OTA Runner that handles the device information.
        """
