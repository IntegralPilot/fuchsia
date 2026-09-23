# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for bluetooth_common_using_sl4f.py."""

import unittest
from collections.abc import Callable
from typing import Any, cast
from unittest import mock

import fidl_fuchsia_bluetooth as f_bt
from honeydew import affordances_capable
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common_using_sl4f,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bluetooth_errors,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)
from honeydew.transports.sl4f import errors as sl4f_errors
from honeydew.transports.sl4f import sl4f as sl4f_transport
from honeydew.typing.custom_types import MacAddress
from parameterized import param, parameterized

_SAMPLE_ADDRESS_OUTPUT: dict[str, Any] = {
    "id": "",
    "result": "[address (public) 20:1F:3B:62:E9:D2]",
    "error": None,
}

_SAMPLE_KNOWN_DEVICES_OUTPUT: dict[str, Any] = {
    "id": "",
    "result": {
        "16085008211800713200": {
            "address": [88, 111, 107, 249, 15, 248],
            "appearance": None,
            "bonded": True,
            "connected": True,
            "device_class": 2097408,
            "id": "16085008211800713200",
            "name": "fuchsia-f80f-f96b-6f59",
            "rssi": -17,
            "services": None,
            "technology": 2,
            "tx_power": None,
        }
    },
    "error": None,
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = cast(Any, testcase_func).__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_with_{test_label}"


BluetoothAcceptPairing = bluetooth_types.BluetoothAcceptPairing
BluetoothConnectionType = bluetooth_types.BluetoothConnectionType


# pylint: disable=protected-access
class BluetoothGapSL4FTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for bluetooth_common_using_sl4f.py."""

    def setUp(self) -> None:
        super().setUp()

        self.sl4f_obj = mock.MagicMock(spec=sl4f_transport.SL4F)
        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice
        )

        self.bluetooth_common_sl4f_obj = (
            bluetooth_common_using_sl4f.BluetoothCommonUsingSl4f(
                device_name="fuchsia-emulator",
                sl4f=self.sl4f_obj,
                reboot_affordance=self.reboot_affordance_obj,
            )
        )

        self.sl4f_obj.run.assert_called()
        self.sl4f_obj.reset_mock()

    def test_sys_init(self) -> None:
        """Test for Bluetooth.sys_init() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            self.bluetooth_common_sl4f_obj.sys_init()
        self.sl4f_obj.run.assert_called()

    async def test_accept_pairing(self) -> None:
        """Test for Bluetooth.accept_pairing() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            await self.bluetooth_common_sl4f_obj.accept_pairing(
                BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
                BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
            )
        self.sl4f_obj.run.assert_called()

    @parameterized.expand(
        [
            (
                {
                    "label": "pair_classic",
                    "transport": BluetoothConnectionType.CLASSIC,
                },
            ),
            (
                {
                    "label": "pair_low_energy",
                    "transport": BluetoothConnectionType.LOW_ENERGY,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_connect_device(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for Bluetooth.connect_device() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            fake_identifier = f_bt.PeerId(value=0)
            await self.bluetooth_common_sl4f_obj.connect_device(
                identifier=fake_identifier,
                connection_type=parameterized_dict["transport"],
            )
        self.sl4f_obj.run.assert_called()

    async def test_forget_device(self) -> None:
        """Test for Bluetooth.forget_device() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            fake_identifier = f_bt.PeerId(value=0)
            await self.bluetooth_common_sl4f_obj.forget_device(fake_identifier)
        self.sl4f_obj.run.assert_called()

    async def test_get_active_adapter_address_success(self) -> None:
        """Test for Bluetooth.get_active_adapter_address() method."""
        self.sl4f_obj.run.return_value = _SAMPLE_ADDRESS_OUTPUT
        res = await self.bluetooth_common_sl4f_obj.get_active_adapter_address()
        self.sl4f_obj.run.assert_called()
        self.assertEqual(res, MacAddress("20:1F:3B:62:E9:D2"))

    async def test_get_active_adapter_address_fail(self) -> None:
        """Test for Bluetooth.get_active_adapter_address() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            _ = (
                await self.bluetooth_common_sl4f_obj.get_active_adapter_address()
            )
        self.sl4f_obj.run.assert_called()

    async def test_get_connected_devices_pass(self) -> None:
        """Test for Bluetooth.get_connected_devices() method."""
        self.sl4f_obj.run.return_value = _SAMPLE_KNOWN_DEVICES_OUTPUT
        res = await self.bluetooth_common_sl4f_obj.get_connected_devices()
        self.sl4f_obj.run.assert_called()
        self.assertEqual(
            res[0],
            _SAMPLE_KNOWN_DEVICES_OUTPUT["result"]["16085008211800713200"][
                "id"
            ],
        )

    async def test_get_connected_devices_fail(self) -> None:
        """Test for Bluetooth.get_connected_devices() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            _ = await self.bluetooth_common_sl4f_obj.get_connected_devices()
        self.sl4f_obj.run.assert_called()

    async def test_get_known_remote_devices_pass(self) -> None:
        """Test for Bluetooth.get_known_remote_devices() method."""
        self.sl4f_obj.run.return_value = _SAMPLE_KNOWN_DEVICES_OUTPUT
        res = await self.bluetooth_common_sl4f_obj.get_known_remote_devices()
        self.sl4f_obj.run.assert_called()
        self.assertEqual(
            res[MacAddress("58:6F:6B:F9:0F:F8")].id.value,
            int(
                _SAMPLE_KNOWN_DEVICES_OUTPUT["result"]["16085008211800713200"][
                    "id"
                ]
            ),
        )

    async def test_get_known_remote_devices_fail(self) -> None:
        """Test for Bluetooth.get_known_remote_devices() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            _ = await self.bluetooth_common_sl4f_obj.get_known_remote_devices()
        self.sl4f_obj.run.assert_called()

    @parameterized.expand(
        [
            (
                {
                    "label": "pair_classic",
                    "transport": BluetoothConnectionType.CLASSIC,
                },
            ),
            (
                {
                    "label": "pair_low_energy",
                    "transport": BluetoothConnectionType.LOW_ENERGY,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_pair_device(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for Bluetooth.pair_device() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            fake_identifier = f_bt.PeerId(value=0)
            await self.bluetooth_common_sl4f_obj.pair_device(
                identifier=fake_identifier,
                connection_type=parameterized_dict["transport"],
            )
        self.sl4f_obj.run.assert_called()

    @parameterized.expand(
        [
            ({"label": "discovery_true", "discovery": True},),
            ({"label": "discovery_false", "discovery": False},),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_request_discovery(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for Bluetooth.request_discovery() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            await self.bluetooth_common_sl4f_obj.request_discovery(
                discovery=parameterized_dict["discovery"]
            )
        self.sl4f_obj.run.assert_called()

    @parameterized.expand(
        [
            ({"label": "set_discoverable_true", "discoverable": True},),
            ({"label": "set_discoverable_false", "discoverable": False},),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_set_discoverable(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for Bluetooth.set_discoverable() method."""
        self.sl4f_obj.run.side_effect = sl4f_errors.Sl4fError("fail")
        with self.assertRaises(bluetooth_errors.BluetoothError):
            await self.bluetooth_common_sl4f_obj.set_discoverable(
                discoverable=parameterized_dict["discoverable"]
            )
        self.sl4f_obj.run.assert_called()


if __name__ == "__main__":
    unittest.main()
