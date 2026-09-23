# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.fuchsia_controller.bluetooth.fidl_servers.bt_fidl_servers.py"""

import unittest
from unittest import mock

import fidl_fuchsia_bluetooth as f_bt
import fidl_fuchsia_bluetooth_sys as f_btsys_controller
from fuchsia_controller_py import Channel
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bluetooth_errors,
)
from honeydew.affordances.connectivity.bluetooth.utils.fidl_servers.bt_fidl_servers import (
    PairingDelegateImpl,
)

_SAMPLE_ON_PAIRING_REQUEST_INPUT = (
    f_btsys_controller.PairingDelegateOnPairingRequestRequest(
        peer=f_btsys_controller.Peer(id_=f_bt.PeerId(value=123)),
        method=f_btsys_controller.PairingMethod(1),
        displayed_passkey=0,
    )
)
_SAMPLE_ON_PAIRING_REQUEST_OUTPUT = (
    f_btsys_controller.PairingDelegateOnPairingRequestResponse(
        accept=True, entered_passkey=0
    )
)
_SAMPLE_ON_PAIRING_COMPLETE_SUCCESS_INPUT = (
    f_btsys_controller.PairingDelegateOnPairingCompleteRequest(
        id_=f_bt.PeerId(value=123), success=True
    )
)
_SAMPLE_ON_PAIRING_COMPLETE_FAILURE_INPUT = (
    f_btsys_controller.PairingDelegateOnPairingCompleteRequest(
        id_=f_bt.PeerId(value=123), success=False
    )
)


class BluetoothFidlServerTest(unittest.TestCase):
    def setUp(self) -> None:
        super().setUp()
        self.channel_mock_object = mock.MagicMock(spec=Channel)
        self.bluetooth_server = PairingDelegateImpl(
            channel=self.channel_mock_object,
        )

    def test_on_pairing_request(self) -> None:
        """Test for PairingDelegateImpl.on_pairing_request() method."""
        data = self.bluetooth_server.on_pairing_request(
            request=_SAMPLE_ON_PAIRING_REQUEST_INPUT
        )
        self.assertEqual(data, _SAMPLE_ON_PAIRING_REQUEST_OUTPUT)

    def test_on_pairing_complete_success(self) -> None:
        """Test for PairingDelegateImpl.on_pairing_complete() method."""
        # The test passes by virtue of not raising an exception.
        self.bluetooth_server.on_pairing_complete(
            request=_SAMPLE_ON_PAIRING_COMPLETE_SUCCESS_INPUT
        )

    def test_on_pairing_complete_failure(self) -> None:
        """Test for PairingDelegateImpl.on_pairing_complete() method."""
        with self.assertRaises(bluetooth_errors.BluetoothError):
            self.bluetooth_server.on_pairing_complete(
                request=_SAMPLE_ON_PAIRING_COMPLETE_FAILURE_INPUT
            )
