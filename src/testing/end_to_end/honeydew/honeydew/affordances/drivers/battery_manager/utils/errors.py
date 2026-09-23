# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Errors raised by Battery affordance."""

import fidl_fuchsia_hardware_power_source as f_power_source
from honeydew import errors


class HoneydewBatteryError(errors.HoneydewError):
    """Base error raised by Battery affordance."""


class BatteryRequestError(HoneydewBatteryError):
    """Raised when the battery service or driver returns an error code."""

    def __init__(self, method: str, error: f_power_source.Error | int) -> None:
        err: f_power_source.Error | int
        try:
            err = f_power_source.Error(error)
            err_str = f"{err.name} ({err.value})"
        except ValueError:
            err = error
            err_str = str(error)
        super().__init__(f"{method} failed with error {err_str}")
        self.error = err


class BatteryDeviceNotFoundError(HoneydewBatteryError):
    """Raised when the battery hardware node is not found on the device."""
