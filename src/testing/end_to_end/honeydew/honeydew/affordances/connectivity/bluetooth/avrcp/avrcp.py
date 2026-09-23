# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Bluetooth AVRCP Profile affordance."""

import abc
from typing import Any

import fidl_fuchsia_bluetooth as f_bt
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)


class Avrcp(bluetooth_common.BluetoothCommon):
    """Abstract base class for an async Bluetooth AVRCP Profile affordance."""

    # List all the public methods
    @abc.abstractmethod
    async def init_avrcp(self, target_id: f_bt.PeerId) -> None:
        """Initialize AVRCP service from the sink device."""

    @abc.abstractmethod
    async def list_received_requests(self) -> list[Any]:
        """List received requests received from source device."""

    @abc.abstractmethod
    async def publish_mock_player(self) -> None:
        """Publish the media session mock player."""

    @abc.abstractmethod
    async def send_avrcp_command(
        self, command: bluetooth_types.BluetoothAvrcpCommand
    ) -> None:
        """Send Avrcp command from the sink device."""

    @abc.abstractmethod
    async def stop_mock_player(self) -> None:
        """Stop the media session mock player."""
