# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by WLAN affordances."""

from honeydew import errors


class BluetoothError(errors.HoneydewError):
    """Exception to be raised if Bluetooth operation fails."""


class BluetoothStateError(errors.HoneydewError):
    """Exception to be raised for unexpected Bluetooth states."""
