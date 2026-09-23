# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Bluetooth FIDL Server Implementations for Fuchsia Controller affordances"""

import logging
from typing import Any

import fidl_fuchsia_bluetooth_gatt2 as f_gatt_controller
import fidl_fuchsia_bluetooth_sys as f_btsys_controller
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bt_errors,
)

_LOGGER: logging.Logger = logging.getLogger(__name__)


class PairingDelegateImpl(f_btsys_controller.PairingDelegateServer):
    """Pairing Delegate Server Implementation follows the FIDL SDK
    fuchsia.bluetooth.sys/pairing.fidl:PairingDelegate spec.
    """

    def on_pairing_request(
        self,
        request: f_btsys_controller.PairingDelegateOnPairingRequestRequest,
    ) -> f_btsys_controller.PairingDelegateOnPairingRequestResponse:
        """On Pairing Request implementation for Pairing Delegate Server

        Args:
            request: pairing request that Bluetooth stack received.

        Returns:
            response: pairing response to Bluetooth stack.
        """
        assert request.peer.id_ is not None
        _LOGGER.info(
            "On Pairing Request method called with peer: %s",
            request.peer.id_.value,
        )
        return f_btsys_controller.PairingDelegateOnPairingRequestResponse(
            accept=True, entered_passkey=0
        )

    def on_pairing_complete(
        self,
        request: f_btsys_controller.PairingDelegateOnPairingCompleteRequest,
    ) -> None:
        """On Pairing Complete implementation for Pairing Delegate Server

        Args:
            request: pairing response completion request from Bluetooth stack.

        Raises:
            BluetoothError: Pairing request failed to complete from the device.
        """
        if not request.success:
            raise bt_errors.BluetoothError("Pairing request failed.")
        _LOGGER.info("Pairing was successful.")

    def on_remote_keypress(self, *args: Any, **kwargs: Any) -> None:
        raise NotImplementedError(
            "Honeydew PairingDelegateImpl does not implement PairingDelegate.OnRemoteKeypress"
        )


class GattLocalServerImpl(f_gatt_controller.LocalServiceServer):
    """Gatt Local Server Implementation follows the FIDL SDK
    fuchsia.bluetooth.gatt2/server.fidl:LocalService spec.
    """

    def read_value(
        self, request: f_gatt_controller.LocalServiceReadValueRequest
    ) -> f_gatt_controller.LocalServiceReadValueResponse:
        """Read value implementation for Local Server implementation

        Args:
            request: a request to read the value on the Gatt Service

        Return:
            [1, 2, 3]: a list of ints representing mock values
        """
        _LOGGER.info(
            "Reading value request from peer: %s",
            request.peer_id.value,
        )
        return f_gatt_controller.LocalServiceReadValueResponse(value=[1, 2, 3])
