# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import types
import unittest
from typing import TypeVar
from unittest import mock

import fidl_fuchsia_wlan_common as f_wlan_common
import fidl_fuchsia_wlan_device_service as f_wlan_device_service
import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_controller_py as fc
from fuchsia_controller_py import Channel, Context, FcTransportStatus
from honeydew import affordances_capable
from honeydew.affordances.connectivity.wlan import wlan_policy_ap
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    AccessPointState,
    NetworkIdentifier,
)
from honeydew.errors import NotSupportedError
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)

_TEST_SSID = "ThepromisedLAN"
_TEST_SSID_BYTES = list(str.encode(_TEST_SSID))

_ACCESS_POINT_STATE = AccessPointState(
    state=f_wlan_policy.OperatingState.STARTING,
    mode=f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
    band=f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
    frequency=None,
    clients=None,
    id_=NetworkIdentifier(
        ssid=_TEST_SSID, security_type=f_wlan_policy.SecurityType.WPA2
    ),
)
_ACCESS_POINT_STATE_FIDL = f_wlan_policy.AccessPointState(
    state=f_wlan_policy.OperatingState.STARTING,
    mode=f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
    band=f_wlan_policy.OperatingBand.ONLY_2_4_GHZ,
    frequency=None,
    clients=None,
    id_=f_wlan_policy.NetworkIdentifier(
        ssid=list(_TEST_SSID_BYTES),
        type_=f_wlan_policy.SecurityType.WPA2,
    ),
)


_T = TypeVar("_T")


async def _async_response(response: _T) -> _T:
    return response


async def _async_error(err: Exception) -> None:
    raise err


# pylint: disable=protected-access
class WlanPolicyApTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()

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
        ) -> tuple[fc.Channel, fc.Channel]:
            return self.ctx.channel_create()

        self.fc_transport_obj.channel_create = types.MethodType(
            channel_create, self.fc_transport_obj
        )
        self.ffx_transport_obj = mock.MagicMock(
            spec=ffx_transport.FFX,
            autospec=True,
        )

        self.ffx_transport_obj.run.return_value = "".join(
            wlan_policy_ap._REQUIRED_CAPABILITIES
        )

        self.access_point_state_updates_proxy: (
            f_wlan_policy.AccessPointStateUpdatesClient | None
        ) = None

        self.ap_provider_proxy = mock.MagicMock(
            spec=f_wlan_policy.AccessPointProviderClient,
            autospec=True,
        )

        # pylint: disable-next=unused-argument
        def get_controller(requests: Channel, updates: Channel) -> None:
            self.access_point_state_updates_proxy = (
                f_wlan_policy.AccessPointStateUpdatesClient(updates)
            )

        self.ap_provider_proxy.get_controller = mock.Mock(wraps=get_controller)

        self.ap_listener_proxy = mock.MagicMock(
            spec=f_wlan_policy.AccessPointListenerClient,
            autospec=True,
        )

        def get_listener(updates: Channel) -> None:
            self.access_point_state_updates_proxy = (
                f_wlan_policy.AccessPointStateUpdatesClient(updates)
            )

        self.ap_listener_proxy.get_listener = mock.Mock(wraps=get_listener)

        self.access_point_controller_obj = mock.MagicMock(
            spec=f_wlan_policy.AccessPointControllerClient,
            autospec=True,
        )

        self.device_monitor_proxy = mock.MagicMock(
            spec=f_wlan_device_service.DeviceMonitorClient,
            autospec=True,
        )
        self.device_monitor_proxy.list_phys.return_value = _async_response(
            f_wlan_device_service.DeviceMonitorListPhysResponse(phy_list=[1])
        )

        mock_get_supported_mac_roles_response = mock.Mock()
        mock_get_supported_mac_roles_response.unwrap.return_value.supported_mac_roles = [
            f_wlan_common.WlanMacRole.AP
        ]

        self.device_monitor_proxy.get_supported_mac_roles.return_value = (
            _async_response(mock_get_supported_mac_roles_response)
        )

        for target, return_value in [
            (
                "fidl_fuchsia_wlan_policy.AccessPointProviderClient",
                self.ap_provider_proxy,
            ),
            (
                "fidl_fuchsia_wlan_policy.AccessPointListenerClient",
                self.ap_listener_proxy,
            ),
            (
                "fidl_fuchsia_wlan_policy.AccessPointControllerClient",
                self.access_point_controller_obj,
            ),
            (
                "fidl_fuchsia_wlan_device_service.DeviceMonitorClient",
                self.device_monitor_proxy,
            ),
        ]:
            patcher = mock.patch(target, return_value=return_value)
            patcher.start()
            self.addCleanup(patcher.stop)

        self.wlan_policy_ap_obj = wlan_policy_ap.WlanPolicyAp(
            device_name="fuchsia-emulator",
            ffx=self.ffx_transport_obj,
            fuchsia_controller=self.fc_transport_obj,
            reboot_affordance=self.reboot_affordance_obj,
            fuchsia_device_close=self.fuchsia_device_close_obj,
        )

        # Call make_ready() to ensure the affordance is initialized. This is
        # necessary because some tests push updates through the update server
        # task which is created during initialization. Normally make_ready()
        # is called lazily via the @ensure_ready decorator.
        await self.wlan_policy_ap_obj.make_ready()

        assert self.access_point_state_updates_proxy is not None

    async def test_verify_supported(self) -> None:
        """Verify verify_supported fails."""
        self.ffx_transport_obj.run.return_value = ""

        with self.assertRaises(NotSupportedError):
            wlan_policy_ap.WlanPolicyAp(
                device_name="fuchsia-emulator",
                ffx=self.ffx_transport_obj,
                fuchsia_controller=self.fc_transport_obj,
                reboot_affordance=self.reboot_affordance_obj,
                fuchsia_device_close=self.fuchsia_device_close_obj,
            )

    async def test_init_register_for_on_device_boot(self) -> None:
        """Verify WlanPolicyAp registers on_device_boot."""
        self.reboot_affordance_obj.register_for_on_device_boot.assert_called_once()

    async def test_init_connect_proxy(self) -> None:
        """Verify WlanPolicyAp connects to
        fuchsia.wlan.policy/AccessPointProvider."""
        self.assertIsNotNone(self.wlan_policy_ap_obj._access_point_controller)

    async def test_start(self) -> None:
        """Verify WlanPolicyAp.start()."""
        self.access_point_controller_obj.start_access_point.side_effect = [
            _async_response(
                f_wlan_policy.AccessPointControllerStartAccessPointResponse(
                    status=f_wlan_policy.RequestStatus.ACKNOWLEDGED
                )
            )
        ]

        await self.wlan_policy_ap_obj.start(
            _TEST_SSID,
            f_wlan_policy.SecurityType.NONE,
            None,
            f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
            f_wlan_policy.OperatingBand.ANY,
        )

    async def test_start_fails(self) -> None:
        """Verify WlanPolicyAp.start() throws HoneydewWlanError on internal error."""
        self.access_point_controller_obj.start_access_point.side_effect = [
            _async_error(FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL))
        ]

        with self.assertRaises(HoneydewWlanError):
            await self.wlan_policy_ap_obj.start(
                _TEST_SSID,
                f_wlan_policy.SecurityType.NONE,
                None,
                f_wlan_policy.ConnectivityMode.LOCAL_ONLY,
                f_wlan_policy.OperatingBand.ANY,
            )

    async def test_stop(self) -> None:
        """Verify WlanPolicyAp.stop()."""
        self.access_point_controller_obj.stop_access_point.side_effect = [
            _async_response(
                f_wlan_policy.AccessPointControllerStartAccessPointResponse(
                    status=f_wlan_policy.RequestStatus.ACKNOWLEDGED
                )
            )
        ]

        await self.wlan_policy_ap_obj.stop(
            _TEST_SSID,
            f_wlan_policy.SecurityType.NONE,
            None,
        )

    async def test_stop_fails(self) -> None:
        """Verify WlanPolicyAp.stop() throws HoneydewWlanError on internal error."""
        self.access_point_controller_obj.stop_access_point.side_effect = [
            _async_error(FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL))
        ]

        with self.assertRaises(HoneydewWlanError):
            await self.wlan_policy_ap_obj.stop(
                _TEST_SSID,
                f_wlan_policy.SecurityType.NONE,
                None,
            )

    async def test_stop_all(self) -> None:
        """Verify WlanPolicyAp.stop_all()."""
        await self.wlan_policy_ap_obj.stop_all()

    async def test_set_new_update_listener_overrides(self) -> None:
        """Verify WlanPolicyAp.set_new_update_listener() overrides the existing
        access point state updates server."""
        assert self.wlan_policy_ap_obj._access_point_controller is not None
        old_server = (
            self.wlan_policy_ap_obj._access_point_controller.access_point_state_updates_server_task
        )

        await self.wlan_policy_ap_obj.set_new_update_listener()

        new_server = (
            self.wlan_policy_ap_obj._access_point_controller.access_point_state_updates_server_task
        )

        self.assertNotEqual(new_server, old_server)
        self.assertTrue(old_server.cancelled())
        self.assertFalse(new_server.cancelled())
        self.assertEqual(
            self.wlan_policy_ap_obj._access_point_controller.updates.qsize(),
            0,
        )

    async def test_get_update(self) -> None:
        """Verify WlanPolicyAp.get_update()."""
        self.assertIsNotNone(self.access_point_state_updates_proxy)
        assert self.access_point_state_updates_proxy is not None

        (
            await self.access_point_state_updates_proxy.on_access_point_state_update(
                access_points=[
                    _ACCESS_POINT_STATE_FIDL,
                ]
            )
        )
        self.assertEqual(
            await self.wlan_policy_ap_obj.get_update(), [_ACCESS_POINT_STATE]
        )

    async def test_get_update_queuing(self) -> None:
        """Verify WlanPolicyAp.get_update() queues updates."""
        self.assertIsNotNone(self.access_point_state_updates_proxy)
        assert self.access_point_state_updates_proxy is not None

        (
            await self.access_point_state_updates_proxy.on_access_point_state_update(
                access_points=[]
            )
        )
        (
            await self.access_point_state_updates_proxy.on_access_point_state_update(
                access_points=[_ACCESS_POINT_STATE_FIDL]
            )
        )
        self.assertEqual(await self.wlan_policy_ap_obj.get_update(), [])
        self.assertEqual(
            await self.wlan_policy_ap_obj.get_update(), [_ACCESS_POINT_STATE]
        )

    @mock.patch(
        "asyncio.wait_for", autospec=True, side_effect=[asyncio.TimeoutError]
    )
    async def test_get_update_timeout(
        self, wait_for_mock: mock.MagicMock
    ) -> None:
        """Verify WlanPolicyAp.get_update() throws TimeoutError on timeout."""
        with self.assertRaises(TimeoutError):
            await self.wlan_policy_ap_obj.get_update(timeout=10)
        wait_for_mock.assert_called_once()


if __name__ == "__main__":
    unittest.main()
