#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion.event.event import Event


class AndroidEvent(Event):
    """The base class for AndroidDevice-related events."""

    def __init__(self, android_device):
        self.android_device = android_device

    @property
    def ad(self):
        return self.android_device


class AndroidStartServicesEvent(AndroidEvent):
    """The event posted when an AndroidDevice begins its services."""


class AndroidStopServicesEvent(AndroidEvent):
    """The event posted when an AndroidDevice ends its services."""


class AndroidRebootEvent(AndroidEvent):
    """The event posted when an AndroidDevice has rebooted."""


class AndroidDisconnectEvent(AndroidEvent):
    """The event posted when an AndroidDevice has disconnected."""


class AndroidReconnectEvent(AndroidEvent):
    """The event posted when an AndroidDevice has disconnected."""


class AndroidBugReportEvent(AndroidEvent):
    """The event posted when an AndroidDevice captures a bugreport."""

    def __init__(self, android_device, bugreport_dir):
        super().__init__(android_device)
        self.bugreport_dir = bugreport_dir
