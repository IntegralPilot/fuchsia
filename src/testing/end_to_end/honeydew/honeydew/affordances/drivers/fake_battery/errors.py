# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Errors raised by FakeBattery affordance."""

from honeydew import errors


class HoneydewFakeBatteryError(errors.HoneydewError):
    """Base error raised by FakeBattery affordance."""


class FakeBatteryCommandError(HoneydewFakeBatteryError):
    """Raised when fake-battery-cli command execution fails."""
