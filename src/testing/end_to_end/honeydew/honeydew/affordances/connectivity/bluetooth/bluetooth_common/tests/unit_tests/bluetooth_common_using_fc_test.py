# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
# pylint: disable=protected-access
"""Unit tests for bluetooth_common_using_fc.py"""

import asyncio
import types
import unittest
from collections.abc import Callable
from typing import Any, cast
from unittest import mock

import fidl_fuchsia_bluetooth as f_bt
import fidl_fuchsia_bluetooth_sys as f_btsys_controller
import fuchsia_controller_py as fc
from honeydew import affordances_capable
from honeydew.affordances.connectivity.bluetooth.bluetooth_common import (
    bluetooth_common_using_fc,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    errors as bluetooth_errors,
)
from honeydew.affordances.connectivity.bluetooth.utils import (
    types as bluetooth_types,
)
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import MacAddress
from parameterized import param, parameterized

BluetoothAcceptPairing = bluetooth_types.BluetoothAcceptPairing
BluetoothConnectionType = bluetooth_types.BluetoothConnectionType

_SAMPLE_KNOWN_DEVICES_OUTPUT = f_btsys_controller.AccessWatchPeersResponse(
    updated=[
        f_btsys_controller.Peer(
            id_=f_bt.PeerId(value=16085008211800713200),
            address=f_bt.Address(
                bytes_=[88, 111, 107, 249, 15, 248], type_=f_bt.AddressType(1)
            ),
            technology=f_btsys_controller.TechnologyType(2),
            connected=True,
            bonded=True,
            name=f_bt.DeviceName("fuchsia-f80f-f96b-6f59"),
            rssi=17,
        ),
    ],
    removed=[
        f_bt.PeerId(value=0),
    ],
)

_ACTUAL_KNOWN_DEVICE_OUTPUT = {
    MacAddress("58:6f:6b:f9:0f:f8"): bluetooth_types.BluetoothPeerInfo(
        id=f_bt.PeerId(value=16085008211800713200),
        address=[88, 111, 107, 249, 15, 248],
        connected=True,
        bonded=True,
        name="fuchsia-f80f-f96b-6f59",
        appearance=None,
        rssi=17,
        services=None,
        technology=2,
        tx_power=None,
    )
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = cast(Any, testcase_func).__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_{test_label}"


class BluetoothCommonFCTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for bluetooth_common_using_fc.py."""

    def setUp(self) -> None:
        super().setUp()
        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController
        )
        self.fc_transport_obj.ctx = fc.Context()

        def channel_create(
            self: fc_transport.FuchsiaController,
        ) -> tuple[fc.Channel, fc.Channel]:
            return self.ctx.channel_create()

        self.fc_transport_obj.channel_create = types.MethodType(
            channel_create, self.fc_transport_obj
        )

        self.bluetooth_common_fc_obj = (
            bluetooth_common_using_fc.BluetoothCommonUsingFc(
                device_name="fuchsia-emulator",
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
            )
        )

    @parameterized.expand(
        [
            (
                {
                    "label": "when_session_not_initialized",
                    "session_initialized": False,
                },
            ),
            (
                {
                    "label": "when_session_already_initialized",
                    "session_initialized": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    def test_sys_init(self, parameterized_dict: dict[str, Any]) -> None:
        """Test for BluetoothGap.sys_init() method."""
        # Check whether an `BluetoothError` exception is raised when
        # calling `sys_init()` on a session that is already initialized.
        if parameterized_dict.get("session_initialized"):
            with self.assertRaises(bluetooth_errors.BluetoothStateError):
                self.bluetooth_common_fc_obj.sys_init()
        else:
            assert (
                self.bluetooth_common_fc_obj._access_controller_proxy
                is not None
            )
            assert (
                self.bluetooth_common_fc_obj._host_watcher_controller_proxy
                is not None
            )
            assert (
                self.bluetooth_common_fc_obj._pairing_controller_proxy
                is not None
            )
            assert not self.bluetooth_common_fc_obj.known_devices

    async def test_reset_state(self) -> None:
        """Test for BluetoothGap.reset_state() method."""
        # Neither MagicMock or AsyncMock are suitable for mocking an asyncio.Task.
        # Use an asyncio.Future() instead since the base implementation is sufficient
        # for this test.
        self.bluetooth_common_fc_obj._pairing_delegate_server = cast(
            Any, asyncio.Future()
        )

        await self.bluetooth_common_fc_obj.reset_state()
        assert self.bluetooth_common_fc_obj._access_controller_proxy is None
        assert (
            self.bluetooth_common_fc_obj._host_watcher_controller_proxy is None
        )
        assert self.bluetooth_common_fc_obj._pairing_controller_proxy is None
        assert self.bluetooth_common_fc_obj._session_initialized is False
        assert self.bluetooth_common_fc_obj._pairing_delegate_server is None

    async def test_accept_pairing(self) -> None:
        """Test for BluetoothGap.accept_pairing() method."""
        self.bluetooth_common_fc_obj._pairing_controller_proxy = (
            mock.MagicMock()
        )
        await self.bluetooth_common_fc_obj.accept_pairing(
            BluetoothAcceptPairing.DEFAULT_INPUT_MODE,
            BluetoothAcceptPairing.DEFAULT_OUTPUT_MODE,
        )
        assert self.bluetooth_common_fc_obj._pairing_delegate_server is not None
        self.bluetooth_common_fc_obj._pairing_controller_proxy.set_pairing_delegate.assert_called_with(
            input_=1, output=1, delegate=mock.ANY
        )
        self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 1)

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
        """Test for BluetoothGap.connect_device() method."""
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.AsyncMock()
        fake_identifier = f_bt.PeerId(value=0)
        with mock.patch(
            "asyncio.sleep", new_callable=mock.AsyncMock
        ) as mock_sleep:
            await self.bluetooth_common_fc_obj.connect_device(
                identifier=fake_identifier,
                connection_type=parameterized_dict["transport"],
            )
            mock_sleep.assert_called_once_with(10)

        self.bluetooth_common_fc_obj._access_controller_proxy.connect.assert_called_with(
            id_=fake_identifier
        )
        self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 2)

    async def test_forget_device(self) -> None:
        """Test for BluetoothGap.forget_device() method."""
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.AsyncMock()
        fake_identifier = f_bt.PeerId(value=0)
        await self.bluetooth_common_fc_obj.forget_device(
            identifier=fake_identifier,
        )
        self.bluetooth_common_fc_obj._access_controller_proxy.forget.assert_called_with(
            id_=fake_identifier
        )
        self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 1)

    async def test_get_active_adapter_address(self) -> None:
        """Test for BluetoothGap.get_active_adapter_address() method."""
        with mock.patch.object(
            self.bluetooth_common_fc_obj,
            "_get_active_address",
            new_callable=mock.AsyncMock,
            return_value=MacAddress("01:23:45:67:89:ab"),
        ):
            fake_address = (
                await self.bluetooth_common_fc_obj.get_active_adapter_address()
            )
            self.assertEqual(str(fake_address), "01:23:45:67:89:ab")

    async def test_async_get_active_adapter_address(self) -> None:
        """Test for BluetoothGap.get_active_adapter_address() async method."""
        self.bluetooth_common_fc_obj._host_watcher_controller_proxy = (
            mock.MagicMock()
        )
        test = f_btsys_controller.HostWatcherWatchResponse(
            hosts=[
                f_btsys_controller.HostInfo(
                    addresses=[
                        f_bt.Address(
                            bytes_=[88, 111, 107, 249, 15, 248], type_=0
                        )
                    ]
                )
            ]
        )
        self.bluetooth_common_fc_obj._host_watcher_controller_proxy.watch = (
            mock.AsyncMock(return_value=test)
        )
        res = await self.bluetooth_common_fc_obj._get_active_address()
        self.assertEqual(str(res), "58:6f:6b:f9:0f:f8")

    async def test_get_known_remote_devices(self) -> None:
        """Test for BluetoothGap.get_known_remote_devices() method."""
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.MagicMock()
        self.bluetooth_common_fc_obj._access_controller_proxy.watch_peers = (
            mock.AsyncMock(return_value=_SAMPLE_KNOWN_DEVICES_OUTPUT)
        )
        data = await self.bluetooth_common_fc_obj.get_known_remote_devices()
        self.assertEqual(data, _ACTUAL_KNOWN_DEVICE_OUTPUT)
        self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 1)

    async def test_get_connected_devices(self) -> None:
        """Test for BluetoothGap.get_connected_devices() method."""
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.MagicMock()
        self.bluetooth_common_fc_obj._access_controller_proxy.watch_peers = (
            mock.AsyncMock(return_value=_SAMPLE_KNOWN_DEVICES_OUTPUT)
        )
        data = await self.bluetooth_common_fc_obj.get_connected_devices()
        self.assertEqual(data, ["16085008211800713200"])

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
        """Test for BluetoothGap.pair_device() method."""
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.AsyncMock()
        fake_identifier = f_bt.PeerId(value=0)
        with mock.patch(
            "asyncio.sleep", new_callable=mock.AsyncMock
        ) as mock_sleep:
            await self.bluetooth_common_fc_obj.pair_device(
                identifier=fake_identifier,
                connection_type=parameterized_dict["transport"],
            )
            mock_sleep.assert_called_once_with(10)

        fake_options = f_btsys_controller.PairingOptions(
            le_security_level=None, bondable_mode=None, transport=None
        )
        self.bluetooth_common_fc_obj._access_controller_proxy.pair.assert_called_with(
            id_=fake_identifier, options=fake_options
        )
        self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 2)

    @parameterized.expand(
        [
            (
                {
                    "label": "discovery_true_with_token",
                    "discovery": True,
                    "token": True,
                },
            ),
            (
                {
                    "label": "discovery_true_without_token",
                    "discovery": True,
                    "token": False,
                },
            ),
            (
                {
                    "label": "discovery_false_without_token",
                    "discovery": False,
                    "token": False,
                },
            ),
            (
                {
                    "label": "discovery_false_with_token",
                    "discovery": False,
                    "token": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_request_discovery(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for BluetoothGap.request_discovery() method."""
        if parameterized_dict.get("token"):
            self.bluetooth_common_fc_obj.discovery_token = mock.MagicMock()
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.AsyncMock()
        if parameterized_dict.get("token") and parameterized_dict.get(
            "discovery"
        ):
            with self.assertRaises(bluetooth_errors.BluetoothError):
                await self.bluetooth_common_fc_obj.request_discovery(
                    discovery=True
                )
        elif parameterized_dict.get("discovery"):
            await self.bluetooth_common_fc_obj.request_discovery(discovery=True)
            self.bluetooth_common_fc_obj._access_controller_proxy.start_discovery.assert_called_once()
            self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 1)
        else:
            await self.bluetooth_common_fc_obj.request_discovery(
                discovery=False
            )
            assert self.bluetooth_common_fc_obj.discovery_token is None

    @parameterized.expand(
        [
            (
                {
                    "label": "discoverable_true_with_token",
                    "discoverable": True,
                    "token": True,
                },
            ),
            (
                {
                    "label": "discoverable_true_without_token",
                    "discoverable": True,
                    "token": False,
                },
            ),
            (
                {
                    "label": "discoverable_false_without_token",
                    "discoverable": False,
                    "token": False,
                },
            ),
            (
                {
                    "label": "discoverable_false_with_token",
                    "discoverable": False,
                    "token": True,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_set_discoverable(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for BluetoothGap.set_discoverable() method."""
        if parameterized_dict.get("token"):
            self.bluetooth_common_fc_obj.discoverable_token = mock.MagicMock()
        self.bluetooth_common_fc_obj._access_controller_proxy = mock.AsyncMock()
        if not parameterized_dict.get("discoverable"):
            await self.bluetooth_common_fc_obj.set_discoverable(
                discoverable=False
            )
            assert self.bluetooth_common_fc_obj.discoverable_token is None
            return
        if parameterized_dict.get("token"):
            with self.assertRaises(bluetooth_errors.BluetoothError):
                await self.bluetooth_common_fc_obj.set_discoverable(
                    discoverable=True
                )
        else:
            await self.bluetooth_common_fc_obj.set_discoverable(
                discoverable=True
            )
            self.bluetooth_common_fc_obj._access_controller_proxy.make_discoverable.assert_called_once()
            self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 1)

    @parameterized.expand(
        [
            (
                {
                    "label": "with_active_pairing_delegate",
                    "server": True,
                },
            ),
            (
                {
                    "label": "without_active_pairing_delegate",
                    "server": False,
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    async def test_run_pairing_delegate(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Test for BluetoothGap.run_pairing_delegate()."""
        if not parameterized_dict.get("server"):
            with self.assertRaises(bluetooth_errors.BluetoothError):
                await self.bluetooth_common_fc_obj.run_pairing_delegate()
        else:
            f: asyncio.Future[None] = asyncio.Future()
            f.set_result(None)
            self.bluetooth_common_fc_obj._pairing_delegate_server = cast(Any, f)
            await self.bluetooth_common_fc_obj.run_pairing_delegate()
            self.assertEqual(self.bluetooth_common_fc_obj._async_op_count, 1)


if __name__ == "__main__":
    unittest.main()
