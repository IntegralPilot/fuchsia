# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""FakeBattery affordance."""

from honeydew.affordances.drivers.fake_battery.errors import (
    FakeBatteryCommandError,
    HoneydewFakeBatteryError,
)
from honeydew.affordances.drivers.fake_battery.fake_battery import FakeBattery
from honeydew.affordances.drivers.fake_battery.fake_battery_using_ffx import (
    FakeBatteryUsingFfx,
)

__all__ = [
    "FakeBattery",
    "FakeBatteryUsingFfx",
    "HoneydewFakeBatteryError",
    "FakeBatteryCommandError",
]
