# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Bluetooth Gap Profile affordance."""

from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common,
)


class Gap(bluetooth_common.BluetoothCommon):
    """Abstract base class for an async Bluetooth Gap Profile affordance."""
