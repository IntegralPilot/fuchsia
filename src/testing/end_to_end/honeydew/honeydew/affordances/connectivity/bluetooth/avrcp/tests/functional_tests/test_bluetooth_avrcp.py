# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Bluetooth Avrcp affordance."""

import logging

import fidl_fuchsia_bluetooth as f_bt
import fuchsia_base_test
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)
from honeydew.transports.sl4f.errors import Sl4fError
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

BluetoothAvrcpCommand = bluetooth_types.BluetoothAvrcpCommand


class BluetoothAvrcpAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """BluetoothAvrcp affordance tests"""

    async def test_avrcp_init(self) -> None:
        """Test for Bluetooth.avrcp_init() method."""
        await self.dut.bluetooth_avrcp.init_avrcp(
            target_id=f_bt.PeerId(value=0)
        )

    async def test_list_received_requests(self) -> None:
        """Test for Bluetooth.list_received_requests() method."""
        res = await self.dut.bluetooth_avrcp.list_received_requests()
        assert len(res) == 0

    async def test_publish_mock_player(self) -> None:
        """Test for Bluetooth.publish_mock_player() method."""
        await self.dut.bluetooth_avrcp.publish_mock_player()

    async def test_send_avrcp_command(self) -> None:
        """Test for Bluetooth.send_avrcp_command() method."""
        # Currently fails sending commands since we only test single device
        with asserts.assert_raises(Sl4fError):
            await self.dut.bluetooth_avrcp.send_avrcp_command(
                BluetoothAvrcpCommand.PLAY
            )

    async def test_stop_mock_player(self) -> None:
        """Test for Bluetooth.stop_mock_player() method"""
        await self.dut.bluetooth_avrcp.stop_mock_player()


if __name__ == "__main__":
    test_runner.main()
