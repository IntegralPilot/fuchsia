# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import types
import unittest
from datetime import timedelta
from typing import TypeVar
from unittest import mock

import fidl_fuchsia_wlan_device_service as f_wlan_device_service
import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_controller_py
from fidl import GlobalHandleWaker
from fuchsia_controller_py import Channel, Context, FcTransportStatus, ZxStatus
from honeydew import affordances_capable
from honeydew.affordances.connectivity.wlan import wlan_policy
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    ClientStateSummary,
    NetworkConfig,
    NetworkIdentifier,
    NetworkState,
)
from honeydew.affordances.location import Location
from honeydew.errors import NotSupportedError
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types

_TEST_SSID = "ThepromisedLAN"
_TEST_SSID_BYTES = list(str.encode(_TEST_SSID))

_TEST_PASSWORD = "password"
_TEST_PSK = "c9a68e83bfd123d144ec5256bc45682accfb8e8f0561f39f44dd388cba9e86f2"

_TEST_CREDENTIAL_NONE = f_wlan_policy.Credential(none=f_wlan_policy.Empty())

_TEST_CREDENTIAL_PASSWORD = f_wlan_policy.Credential(
    password=list(str.encode(_TEST_PASSWORD))
)

_TEST_CREDENTIAL_PSK = f_wlan_policy.Credential(
    psk=list(bytes.fromhex(_TEST_PSK))
)

_TEST_NETWORK_CONFIG_NONE = NetworkConfig(
    ssid=_TEST_SSID,
    security_type=f_wlan_policy.SecurityType.NONE,
    credential_type="None",
    credential_value="",
)
_TEST_NETWORK_CONFIG_NONE_FIDL = f_wlan_policy.NetworkConfig(
    id_=f_wlan_policy.NetworkIdentifier(
        ssid=_TEST_SSID_BYTES,
        type_=f_wlan_policy.SecurityType.NONE,
    ),
    credential=_TEST_CREDENTIAL_NONE,
)

_TEST_NETWORK_CONFIG_PASSWORD = NetworkConfig(
    ssid=_TEST_SSID,
    security_type=f_wlan_policy.SecurityType.WPA2,
    credential_type="Password",
    credential_value=_TEST_PASSWORD,
)
_TEST_NETWORK_CONFIG_PASSWORD_FIDL = f_wlan_policy.NetworkConfig(
    id_=f_wlan_policy.NetworkIdentifier(
        ssid=_TEST_SSID_BYTES,
        type_=f_wlan_policy.SecurityType.WPA2,
    ),
    credential=_TEST_CREDENTIAL_PASSWORD,
)

_TEST_NETWORK_CONFIG_PSK = NetworkConfig(
    ssid=_TEST_SSID,
    security_type=f_wlan_policy.SecurityType.WPA2,
    credential_type="Psk",
    credential_value=_TEST_PSK,
)
_TEST_NETWORK_CONFIG_PSK_FIDL = f_wlan_policy.NetworkConfig(
    id_=f_wlan_policy.NetworkIdentifier(
        ssid=_TEST_SSID_BYTES,
        type_=f_wlan_policy.SecurityType.WPA2,
    ),
    credential=_TEST_CREDENTIAL_PSK,
)

_TEST_MAC_ADDRESS_BYTES = bytes([1, 35, 69, 103, 137, 171])  # 01:23:45:67:89:ab


def _make_scan_result(ssid: str) -> f_wlan_policy.ScanResult:
    return f_wlan_policy.ScanResult(
        id_=f_wlan_policy.NetworkIdentifier(
            ssid=list(ssid.encode("utf-8")),
            type_=f_wlan_policy.SecurityType.WPA2,
        ),
        entries=[
            f_wlan_policy.Bss(
                bssid=list(_TEST_MAC_ADDRESS_BYTES),
                rssi=0,
                frequency=0,
                timestamp_nanos=0,
            ),
        ],
        compatibility=f_wlan_policy.Compatibility.SUPPORTED,
    )


_T = TypeVar("_T")


async def _async_response(response: _T) -> _T:
    return response


async def _async_error(err: Exception) -> None:
    raise err


# pylint: disable=protected-access
class WlanPolicyTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()

        wakers = GlobalHandleWaker()
        wakers._reset_for_testing()

        self.reboot_affordance_obj = mock.MagicMock(
            spec=affordances_capable.RebootCapableDevice,
            autospec=True,
        )
        self.fuchsia_device_close_obj = mock.MagicMock(
            spec=affordances_capable.FuchsiaDeviceClose,
            autospec=True,
        )
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController,
            autospec=True,
        )
        self.fc_transport_obj.ctx = Context()

        def channel_create(
            self: fc_transport.FuchsiaController,
        ) -> tuple[
            fuchsia_controller_py.Channel, fuchsia_controller_py.Channel
        ]:
            return self.ctx.channel_create()

        self.fc_transport_obj.channel_create = types.MethodType(
            channel_create, self.fc_transport_obj
        )

        self.location_obj = mock.MagicMock(
            spec=Location,
            autospec=True,
        )

        self.ffx_transport_obj = mock.MagicMock(
            spec=ffx_transport.FFX,
            autospec=True,
        )
        self.ffx_transport_obj.run.return_value = "".join(
            wlan_policy._REQUIRED_CAPABILITIES
        )

        self.wlan_policy_obj = wlan_policy.WlanPolicy(
            device_name="fuchsia-emulator",
            ffx=self.ffx_transport_obj,
            fuchsia_controller=self.fc_transport_obj,
            reboot_affordance=self.reboot_affordance_obj,
            fuchsia_device_close=self.fuchsia_device_close_obj,
            location=self.location_obj,
        )

        self.client_provider_proxy = mock.MagicMock(
            spec=f_wlan_policy.ClientProviderClient
        )
        self.client_listener_proxy = mock.MagicMock(
            spec=f_wlan_policy.ClientListenerClient
        )
        self.device_monitor_proxy = mock.MagicMock(
            spec=f_wlan_device_service.DeviceMonitorClient
        )
        self.client_controller_proxy = mock.MagicMock(
            spec=f_wlan_policy.ClientControllerClient
        )

        self.client_state_updates_proxy: (
            f_wlan_policy.ClientStateUpdatesClient | None
        ) = None
        self.scan_result_iterator: asyncio.Task[None] | None = None
        self.network_config_iterator: asyncio.Task[None] | None = None

        def connect_device_proxy(
            fidl_endpoint: custom_types.FidlEndpoint,
        ) -> mock.MagicMock:
            if fidl_endpoint in [
                wlan_policy._CLIENT_PROVIDER_PROXY,
                wlan_policy._DEVICE_MONITOR_PROXY,
                wlan_policy._CLIENT_LISTENER_PROXY,
            ]:
                return mock.MagicMock(spec=Channel)
            raise ValueError(f"Unexpected endpoint: {fidl_endpoint}")

        self.fc_transport_obj.connect_device_proxy.side_effect = (
            connect_device_proxy
        )

        # Create a FIDL client to the ClientStateUpdates server.
        def get_controller(
            # pylint: disable-next=unused-argument
            requests: Channel,
            updates: Channel,
        ) -> None:
            self.client_state_updates_proxy = (
                f_wlan_policy.ClientStateUpdatesClient(updates)
            )

        self.client_provider_proxy.get_controller = mock.Mock(
            wraps=get_controller
        )

        # Create a FIDL client to the ClientListener server.
        def get_listener(updates: Channel) -> None:
            self.client_state_updates_proxy = (
                f_wlan_policy.ClientStateUpdatesClient(updates)
            )

        self.client_listener_proxy.get_listener = mock.Mock(wraps=get_listener)

        for target, return_value in [
            (
                "fidl_fuchsia_wlan_policy.ClientProviderClient",
                self.client_provider_proxy,
            ),
            (
                "fidl_fuchsia_wlan_policy.ClientListenerClient",
                self.client_listener_proxy,
            ),
            (
                "fidl_fuchsia_wlan_device_service.DeviceMonitorClient",
                self.device_monitor_proxy,
            ),
            (
                "fidl_fuchsia_wlan_policy.ClientControllerClient",
                self.client_controller_proxy,
            ),
        ]:
            patcher = mock.patch(target, return_value=return_value)
            patcher.start()
            self.addCleanup(patcher.stop)

        # Call make_ready() to ensure the affordance is initialized. This is
        # necessary because some tests push updates through the update server
        # task which is created during initialization. Normally make_ready()
        # is called lazily via the @ensure_ready decorator.
        await self.wlan_policy_obj.make_ready()

    async def asyncTearDown(self) -> None:
        await self.wlan_policy_obj._close()
        return await super().asyncTearDown()

    def test_verify_supported(self) -> None:
        """Test if verify_supported works."""
        self.ffx_transport_obj.run.return_value = ""

        with self.assertRaises(NotSupportedError):
            wlan_policy.WlanPolicy(
                device_name="fuchsia-emulator",
                ffx=self.ffx_transport_obj,
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
                fuchsia_device_close=self.fuchsia_device_close_obj,
                location=self.location_obj,
            )

    async def test_connect(self) -> None:
        """Test if connect works."""
        client_controller = self.client_controller_proxy
        with mock.patch.object(
            self.wlan_policy_obj,
            "wait_for_network_state",
            autospec=True,
        ) as mock_wait_for_network_state:
            for msg, resp, should_raise in [
                (
                    "acknowledged",
                    _async_response(
                        f_wlan_policy.ClientControllerConnectResponse(
                            status=f_wlan_policy.RequestStatus.ACKNOWLEDGED
                        )
                    ),
                    False,
                ),
                (
                    "not supported",
                    _async_response(
                        f_wlan_policy.ClientControllerConnectResponse(
                            status=f_wlan_policy.RequestStatus.REJECTED_NOT_SUPPORTED
                        )
                    ),
                    True,
                ),
                (
                    "incompatible mode",
                    _async_response(
                        f_wlan_policy.ClientControllerConnectResponse(
                            status=f_wlan_policy.RequestStatus.REJECTED_INCOMPATIBLE_MODE
                        )
                    ),
                    True,
                ),
                (
                    "already in use",
                    _async_response(
                        f_wlan_policy.ClientControllerConnectResponse(
                            status=f_wlan_policy.RequestStatus.REJECTED_ALREADY_IN_USE
                        )
                    ),
                    True,
                ),
                (
                    "duplicate request",
                    _async_response(
                        f_wlan_policy.ClientControllerConnectResponse(
                            status=f_wlan_policy.RequestStatus.REJECTED_DUPLICATE_REQUEST
                        )
                    ),
                    True,
                ),
                (
                    "internal error",
                    _async_error(
                        FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL)
                    ),
                    True,
                ),
            ]:
                with self.subTest(msg=msg, resp=resp):
                    client_controller.connect.reset_mock()
                    client_controller.connect.return_value = resp
                    mock_wait_for_network_state.reset_mock()

                    if should_raise:
                        with self.assertRaises(HoneydewWlanError):
                            await self.wlan_policy_obj.connect(
                                _TEST_SSID, f_wlan_policy.SecurityType.NONE
                            )
                        mock_wait_for_network_state.assert_not_called()
                    else:
                        await self.wlan_policy_obj.connect(
                            _TEST_SSID, f_wlan_policy.SecurityType.NONE
                        )
                        mock_wait_for_network_state.assert_called_once_with(
                            _TEST_SSID,
                            f_wlan_policy.ConnectionState.CONNECTED,
                            timeout=mock.ANY,
                        )

                    client_controller.connect.assert_called_once()

    async def test_get_saved_networks(self) -> None:
        """Test if get_saved_networks works."""
        client_controller = self.client_controller_proxy

        def get_saved_networks(iterator: int) -> None:
            server = TestNetworkConfigIteratorImpl(
                Channel(iterator),
                items=[
                    [
                        _TEST_NETWORK_CONFIG_NONE_FIDL,
                        _TEST_NETWORK_CONFIG_PASSWORD_FIDL,
                    ],
                    [_TEST_NETWORK_CONFIG_PSK_FIDL],
                ],
            )
            self.network_config_iterator = asyncio.create_task(server.serve())

        client_controller.get_saved_networks = mock.Mock(
            wraps=get_saved_networks
        )

        networks = await self.wlan_policy_obj.get_saved_networks()
        self.assertEqual(
            networks,
            [
                _TEST_NETWORK_CONFIG_NONE,
                _TEST_NETWORK_CONFIG_PASSWORD,
                _TEST_NETWORK_CONFIG_PSK,
            ],
        )

        assert self.network_config_iterator is not None
        self.network_config_iterator.cancel()

    async def test_get_update(self) -> None:
        """Test if get_update works."""
        for msg, fidl, expected in [
            (
                "enabled",
                f_wlan_policy.ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                    networks=[],
                ),
                ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                    networks=[],
                ),
            ),
            (
                "connecting",
                f_wlan_policy.ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                    networks=[
                        f_wlan_policy.NetworkState(
                            id_=f_wlan_policy.NetworkIdentifier(
                                ssid=list(b"Google Guest"),
                                type_=f_wlan_policy.SecurityType.WPA2,
                            ),
                            state=f_wlan_policy.ConnectionState.CONNECTING,
                            status=None,
                        ),
                    ],
                ),
                ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                    networks=[
                        NetworkState(
                            network_identifier=NetworkIdentifier(
                                ssid="Google Guest",
                                security_type=f_wlan_policy.SecurityType.WPA2,
                            ),
                            connection_state=f_wlan_policy.ConnectionState.CONNECTING,
                            disconnect_status=None,
                        )
                    ],
                ),
            ),
            (
                "disabled",
                f_wlan_policy.ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED,
                    networks=[],
                ),
                ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED,
                    networks=[],
                ),
            ),
        ]:
            with self.subTest(msg=msg, fidl=fidl, expected=expected):
                assert self.client_state_updates_proxy is not None
                await self.client_state_updates_proxy.on_client_state_update(
                    summary=fidl,
                )
                self.assertEqual(
                    await self.wlan_policy_obj.get_update(),
                    expected,
                )

    async def test_wait_for_client_state(self) -> None:
        """Test if wait_for_client_state works."""
        fidl_state = f_wlan_policy.ClientStateSummary(
            state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
            networks=[],
        )

        async def push_updates() -> None:
            await asyncio.sleep(0.1)
            assert self.client_state_updates_proxy is not None
            await self.client_state_updates_proxy.on_client_state_update(
                summary=fidl_state
            )

        push_task = asyncio.create_task(push_updates())

        await self.wlan_policy_obj.wait_for_client_state(
            expected_state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
            timeout=timedelta(seconds=5),
        )
        await push_task

    async def test_wait_for_network_state(self) -> None:
        """Test if wait_for_network_state works."""
        fidl_state = f_wlan_policy.ClientStateSummary(
            state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
            networks=[
                f_wlan_policy.NetworkState(
                    id_=f_wlan_policy.NetworkIdentifier(
                        ssid=b"ssid1",
                        type_=f_wlan_policy.SecurityType.NONE,
                    ),
                    state=f_wlan_policy.ConnectionState.CONNECTED,
                )
            ],
        )

        async def push_updates() -> None:
            await asyncio.sleep(0.1)
            assert self.client_state_updates_proxy is not None
            await self.client_state_updates_proxy.on_client_state_update(
                summary=fidl_state
            )

        push_task = asyncio.create_task(push_updates())

        state = await self.wlan_policy_obj.wait_for_network_state(
            ssid="ssid1",
            expected_state=f_wlan_policy.ConnectionState.CONNECTED,
            timeout=timedelta(seconds=5),
        )
        self.assertEqual(state, f_wlan_policy.ConnectionState.CONNECTED)
        await push_task

    async def test__wait_on_update(self) -> None:
        """Test if _wait_on_update returns the summary."""
        fidl_state = f_wlan_policy.ClientStateSummary(
            state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
            networks=[],
        )

        async def push_updates() -> None:
            await asyncio.sleep(0.1)
            assert self.client_state_updates_proxy is not None
            await self.client_state_updates_proxy.on_client_state_update(
                summary=fidl_state
            )

        push_task = asyncio.create_task(push_updates())

        def condition(update: ClientStateSummary) -> bool:
            return (
                update.state
                == f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
            )

        summary = await self.wlan_policy_obj._wait_on_update(
            condition, timeout=timedelta(seconds=5)
        )
        self.assertEqual(
            summary.state, f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
        )
        await push_task

    async def test_remove_all_networks(self) -> None:
        """Test if remove_all_networks works."""
        client_controller = self.client_controller_proxy

        # Mock get_saved_networks
        def get_saved_networks(iterator: int) -> None:
            server = TestNetworkConfigIteratorImpl(
                Channel(iterator),
                items=[
                    [
                        _TEST_NETWORK_CONFIG_NONE_FIDL,
                        _TEST_NETWORK_CONFIG_PASSWORD_FIDL,
                        _TEST_NETWORK_CONFIG_PSK_FIDL,
                    ],
                ],
            )
            self.network_config_iterator = asyncio.create_task(server.serve())

        client_controller.get_saved_networks = mock.Mock(
            wraps=get_saved_networks
        )

        # Mock forget_network, which should be called once for each saved
        # network.
        res = f_wlan_policy.ClientControllerForgetNetworkResult(response=None)
        client_controller.forget_network.side_effect = [
            _async_response(res),
            _async_response(res),
            _async_response(res),
        ]

        # Remove all networks
        await self.wlan_policy_obj.remove_all_networks()
        client_controller.forget_network.assert_has_calls(
            [
                mock.call(id_=_TEST_NETWORK_CONFIG_NONE_FIDL.id_),
                mock.call(id_=_TEST_NETWORK_CONFIG_PASSWORD_FIDL.id_),
                mock.call(id_=_TEST_NETWORK_CONFIG_PSK_FIDL.id_),
            ]
        )

        # Cleanup
        assert self.network_config_iterator is not None
        self.network_config_iterator.cancel()

    async def test_forget_network_passes(self) -> None:
        """Test if forget_network works."""
        client_controller = self.client_controller_proxy
        res = f_wlan_policy.ClientControllerForgetNetworkResult(response=None)
        client_controller.forget_network.return_value = _async_response(res)

        await self.wlan_policy_obj.forget_network(
            _TEST_SSID, f_wlan_policy.SecurityType.NONE
        )
        client_controller.forget_network.assert_called_with(
            id_=_TEST_NETWORK_CONFIG_NONE_FIDL.id_
        )

    async def test_forget_network_fails(self) -> None:
        """Test if forget_network throws HoneydewWlanError as expected."""
        client_controller = self.client_controller_proxy
        with self.subTest(msg="NetworkConfigChangeError"):
            res = f_wlan_policy.ClientControllerForgetNetworkResult(
                err=int(
                    f_wlan_policy.NetworkConfigChangeError.CREDENTIAL_LEN_ERROR
                )
            )
            client_controller.forget_network.return_value = _async_response(res)

            with self.assertRaises(HoneydewWlanError):
                await self.wlan_policy_obj.forget_network(
                    _TEST_SSID, f_wlan_policy.SecurityType.NONE
                )
            client_controller.forget_network.assert_called_once()

        with self.subTest(msg="FcTransportStatus"):
            res = f_wlan_policy.ClientControllerForgetNetworkResult(
                err=int(
                    f_wlan_policy.NetworkConfigChangeError.CREDENTIAL_LEN_ERROR
                )
            )
            client_controller.forget_network.reset_mock()
            client_controller.forget_network.return_value = _async_error(
                FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL)
            )

            with self.assertRaises(HoneydewWlanError):
                await self.wlan_policy_obj.forget_network(
                    _TEST_SSID, f_wlan_policy.SecurityType.NONE
                )
            client_controller.forget_network.assert_called_once()

    async def test_save_network_passes(self) -> None:
        """Test if save_network works."""
        client_controller = self.client_controller_proxy
        res = f_wlan_policy.ClientControllerSaveNetworkResult(response=None)
        client_controller.save_network.return_value = _async_response(res)

        await self.wlan_policy_obj.save_network(
            _TEST_SSID, f_wlan_policy.SecurityType.NONE, None
        )
        client_controller.save_network.assert_called_once()

    async def test_save_network_fails(self) -> None:
        """Test if save_network throws HoneydewWlanError as expected."""
        client_controller = self.client_controller_proxy
        with self.subTest(msg="NetworkConfigChangeError"):
            res = f_wlan_policy.ClientControllerSaveNetworkResult(
                err=int(
                    f_wlan_policy.NetworkConfigChangeError.CREDENTIAL_LEN_ERROR
                )
            )
            client_controller.save_network.return_value = _async_response(res)

            with self.assertRaises(HoneydewWlanError):
                await self.wlan_policy_obj.save_network(
                    _TEST_SSID, f_wlan_policy.SecurityType.NONE, None
                )
            client_controller.save_network.assert_called_once()

        with self.subTest(msg="FcTransportStatus"):
            res = f_wlan_policy.ClientControllerSaveNetworkResult(
                err=int(
                    f_wlan_policy.NetworkConfigChangeError.CREDENTIAL_LEN_ERROR
                )
            )
            client_controller.save_network.reset_mock()
            client_controller.save_network.return_value = _async_error(
                FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL)
            )

            with self.assertRaises(HoneydewWlanError):
                await self.wlan_policy_obj.save_network(
                    _TEST_SSID, f_wlan_policy.SecurityType.NONE, None
                )
            client_controller.save_network.assert_called_once()

    async def test_scan_for_networks(self) -> None:
        """Test if scan_for_networks works."""
        client_controller = self.client_controller_proxy

        def scan_for_networks(iterator: int) -> None:
            server = TestScanResultIteratorImpl(
                Channel(iterator),
                items=[
                    [
                        _TEST_SSID,
                        _TEST_SSID + "2",
                    ],
                    [
                        _TEST_SSID,
                        _TEST_SSID + "3",
                    ],
                ],
            )
            self.scan_result_iterator = asyncio.create_task(server.serve())

        client_controller.scan_for_networks = mock.Mock(wraps=scan_for_networks)

        networks = await self.wlan_policy_obj.scan_for_networks()
        networks.sort()
        expected = list(
            {
                _TEST_SSID,
                _TEST_SSID + "2",
                _TEST_SSID + "3",
            }
        )
        expected.sort()
        self.assertEqual(
            networks,
            expected,
        )

        assert self.scan_result_iterator is not None
        self.scan_result_iterator.cancel()

    async def test_set_new_update_listener_without_client_controller(
        self,
    ) -> None:
        """Test if set_new_update_listener creates a client controller if it
        doesn't already exist."""
        await self.wlan_policy_obj.set_new_update_listener()

        self.assertIsNotNone(self.client_state_updates_proxy)
        self.assertIsNotNone(self.wlan_policy_obj._client_controller)
        assert self.wlan_policy_obj._client_controller is not None
        self.assertEqual(
            self.wlan_policy_obj._client_controller.updates.qsize(), 0
        )

    async def test_set_new_update_listener_overrides(self) -> None:
        """Test if set_new_update_listener overrides an existing client state
        updates server."""
        self.assertIsNotNone(self.wlan_policy_obj._client_controller)
        assert self.wlan_policy_obj._client_controller is not None
        old_server = (
            self.wlan_policy_obj._client_controller.client_state_updates_server_task
        )

        await self.wlan_policy_obj.set_new_update_listener()

        self.assertIsNotNone(self.wlan_policy_obj._client_controller)
        assert self.wlan_policy_obj._client_controller is not None
        new_server = (
            self.wlan_policy_obj._client_controller.client_state_updates_server_task
        )

        self.assertNotEqual(new_server, old_server)
        self.assertTrue(old_server.cancelled())
        self.assertFalse(new_server.cancelled())

    async def test_start_client_connections_passes(self) -> None:
        """Test if start_client_connections passes as expected."""
        client_controller = self.client_controller_proxy
        client_controller.start_client_connections.return_value = (
            _async_response(
                f_wlan_policy.ClientControllerStartClientConnectionsResponse(
                    status=f_wlan_policy.RequestStatus.ACKNOWLEDGED
                )
            )
        )
        await self.wlan_policy_obj.start_client_connections()
        client_controller.start_client_connections.assert_called_once_with()

    async def test_start_client_connections_fails(self) -> None:
        """Test if start_client_connections fails in expected ways."""
        client_controller = self.client_controller_proxy
        for msg, resp in [
            (
                "not supported",
                _async_response(
                    f_wlan_policy.ClientControllerStartClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_NOT_SUPPORTED
                    )
                ),
            ),
            (
                "incompatible mode",
                _async_response(
                    f_wlan_policy.ClientControllerStartClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_INCOMPATIBLE_MODE
                    )
                ),
            ),
            (
                "already in use",
                _async_response(
                    f_wlan_policy.ClientControllerStartClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_ALREADY_IN_USE
                    )
                ),
            ),
            (
                "duplicate request",
                _async_response(
                    f_wlan_policy.ClientControllerStartClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_DUPLICATE_REQUEST
                    )
                ),
            ),
            (
                "internal error",
                _async_error(
                    FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL)
                ),
            ),
        ]:
            with self.subTest(msg=msg, resp=resp):
                client_controller.start_client_connections.reset_mock()
                client_controller.start_client_connections.return_value = resp
                with self.assertRaises(HoneydewWlanError):
                    await self.wlan_policy_obj.start_client_connections()
                client_controller.start_client_connections.assert_called_once_with()

    async def test_stop_client_connections(self) -> None:
        """Test if stop_client_connections passes as expected."""
        client_controller = self.client_controller_proxy
        client_controller.stop_client_connections.return_value = (
            _async_response(
                f_wlan_policy.ClientControllerStopClientConnectionsResponse(
                    status=f_wlan_policy.RequestStatus.ACKNOWLEDGED
                )
            )
        )
        with mock.patch.object(
            self.wlan_policy_obj,
            "get_status",
            mock.AsyncMock(
                return_value=ClientStateSummary(
                    state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                    networks=[],
                )
            ),
        ):
            await self.wlan_policy_obj.stop_client_connections(
                wait_for_confirmation=False
            )
        client_controller.stop_client_connections.assert_called_once_with()

    async def test_stop_client_connections_fails(self) -> None:
        """Test if stop_client_connections fails in expected ways."""
        client_controller = self.client_controller_proxy
        for msg, resp in [
            (
                "not supported",
                _async_response(
                    f_wlan_policy.ClientControllerStopClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_NOT_SUPPORTED
                    )
                ),
            ),
            (
                "incompatible mode",
                _async_response(
                    f_wlan_policy.ClientControllerStopClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_INCOMPATIBLE_MODE
                    )
                ),
            ),
            (
                "already in use",
                _async_response(
                    f_wlan_policy.ClientControllerStopClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_ALREADY_IN_USE
                    )
                ),
            ),
            (
                "duplicate request",
                _async_response(
                    f_wlan_policy.ClientControllerStopClientConnectionsResponse(
                        status=f_wlan_policy.RequestStatus.REJECTED_DUPLICATE_REQUEST
                    )
                ),
            ),
            (
                "internal error",
                _async_error(
                    FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL)
                ),
            ),
        ]:
            with self.subTest(msg=msg, resp=resp):
                client_controller.stop_client_connections.reset_mock()
                client_controller.stop_client_connections.return_value = resp
                with mock.patch.object(
                    self.wlan_policy_obj,
                    "get_status",
                    mock.AsyncMock(
                        return_value=ClientStateSummary(
                            state=f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED,
                            networks=[],
                        )
                    ),
                ):
                    with self.assertRaises(HoneydewWlanError):
                        await self.wlan_policy_obj.stop_client_connections(
                            wait_for_confirmation=False
                        )
                client_controller.stop_client_connections.assert_called_once_with()


class TestScanResultIteratorImpl(f_wlan_policy.ScanResultIteratorServer):
    """Iterator for scan results."""

    def __init__(self, server: Channel, items: list[list[str]]) -> None:
        super().__init__(server)
        self._items = items

    def get_next(self) -> f_wlan_policy.ScanResultIteratorGetNextResponse:
        """Get next set of scan result SSIDs."""
        if len(self._items) == 0:
            raise ZxStatus(ZxStatus.ZX_ERR_PEER_CLOSED)
        return f_wlan_policy.ScanResultIteratorGetNextResponse(
            scan_results=[
                _make_scan_result(ssid) for ssid in self._items.pop(0)
            ],
        )


class TestNetworkConfigIteratorImpl(f_wlan_policy.NetworkConfigIteratorServer):
    """Iterator for NetworkConfig results."""

    def __init__(
        self, server: Channel, items: list[list[f_wlan_policy.NetworkConfig]]
    ) -> None:
        super().__init__(server)
        self._items = items

    def get_next(
        self,
    ) -> f_wlan_policy.NetworkConfigIteratorGetNextResponse:
        """Get next set of NetworkConfigs."""
        if len(self._items) == 0:
            raise ZxStatus(ZxStatus.ZX_ERR_PEER_CLOSED)
        return f_wlan_policy.NetworkConfigIteratorGetNextResponse(
            configs=self._items.pop(0),
        )


if __name__ == "__main__":
    unittest.main()
