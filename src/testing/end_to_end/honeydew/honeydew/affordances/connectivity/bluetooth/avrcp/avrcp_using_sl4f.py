# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""SL4F based implementation for Bluetooth AVRCP Profile affordance."""

from enum import StrEnum

import fidl_fuchsia_bluetooth as f_bt
from honeydew import affordances_capable
from honeydew.affordances.connectivity.bluetooth.avrcp import avrcp
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common_using_sl4f,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)
from honeydew.transports.sl4f import sl4f as sl4f_transport


class Sl4fMethods(StrEnum):
    INIT_AVRCP = "avrcp_facade.AvrcpInit"
    LIST_RECEIVED_REQUESTS = "media_session_facade.ListReceivedRequests"
    PUBLISH_MOCK_PLAYER = "media_session_facade.PublishMockPlayer"
    SEND_AVRCP_COMMAND = "avrcp_facade.AvrcpSendCommand"
    STOP_MOCK_PLAYER = "media_session_facade.StopMockPlayer"


class AvrcpUsingSl4f(
    bluetooth_common_using_sl4f.BluetoothCommonUsingSl4f,
    avrcp.Avrcp,
):
    """SL4F based implementation for BluetoothAvrcp Profile affordance."""

    def __init__(
        self,
        device_name: str,
        sl4f: sl4f_transport.SL4F,
        reboot_affordance: affordances_capable.RebootCapableDevice,
    ) -> None:
        super().__init__(device_name, sl4f, reboot_affordance)
        self.__sl4f = sl4f
        self.verify_supported()

    # List all the public methods
    async def init_avrcp(self, target_id: f_bt.PeerId) -> None:
        """Initialize AVRCP service from the sink device.

        Args:
            target_id: id of source device to start AVRCP

        Raises:
            Sl4fError: On failure.
        """
        self.__sl4f.run(
            method=Sl4fMethods.INIT_AVRCP,
            params={"target_id": str(target_id.value)},
        )

    def verify_supported(self) -> None:
        """Check if Bluetooth avrpc is supported on the DUT.
        Raises:
            NotSupportedError: AVRCP affordance is not supported by Fuchsia device.
        """
        # TODO(http://b/409622631): Implement the method logic

    async def list_received_requests(self) -> list[object]:
        """List received requests received from source device.

        Returns:
            A list of the most recent commands received, where the last
            element in the list is the most recent command received. If no
            result then return empty list.
        Raises:
            errors.Sl4fError: On failure.
        """
        requests = self.__sl4f.run(method=Sl4fMethods.LIST_RECEIVED_REQUESTS)
        return requests.get("result", [])

    async def publish_mock_player(self) -> None:
        """Publish the media session mock player.

        Raises:
            errors.Sl4fError: On failure.
        """
        self.__sl4f.run(method=Sl4fMethods.PUBLISH_MOCK_PLAYER)

    async def send_avrcp_command(
        self, command: bluetooth_types.BluetoothAvrcpCommand
    ) -> None:
        """Send Avrcp command from the sink device.

        Args:
            command: the command to send to the AVRCP service.

        Raises:
            errors.Sl4fError: On Failure.
        """
        self.__sl4f.run(
            method=Sl4fMethods.SEND_AVRCP_COMMAND, params={"command": command}
        )

    async def stop_mock_player(self) -> None:
        """Stop the media session mock player.

        Raises:
            errors.Sl4fError: On Failure.
        """
        self.__sl4f.run(method=Sl4fMethods.STOP_MOCK_PLAYER)
