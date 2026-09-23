# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""WLAN policy access point affordance implementation using Fuchsia
Controller."""

import asyncio
import logging
from dataclasses import dataclass

import fidl_fuchsia_wlan_common as f_wlan_common
import fidl_fuchsia_wlan_device_service as f_wlan_device_service
import fidl_fuchsia_wlan_policy as f_wlan_policy
from fuchsia_controller_py import Channel, FcTransportStatus, ZxStatus
from honeydew import affordances_capable, errors
from honeydew.affordances.affordance import AsyncLazyReady, ensure_ready
from honeydew.affordances.connectivity.wlan.utils import errors as wlan_errors
from honeydew.affordances.connectivity.wlan.utils.types import (
    AccessPointState,
    Credential,
    NetworkConfig,
)
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.ffx import types as ffx_types
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint

# List of required FIDLs for the affordance.
_REQUIRED_CAPABILITIES = [
    "fuchsia.wlan.policy.AccessPointListener",
    "fuchsia.wlan.policy.AccessPointProvider",
]

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Fuchsia Controller proxies
_ACCESS_POINT_PROVIDER_PROXY = FidlEndpoint(
    "core/wlancfg", "fuchsia.wlan.policy.AccessPointProvider"
)
_ACCESS_POINT_LISTENER_PROXY = FidlEndpoint(
    "core/wlancfg", "fuchsia.wlan.policy.AccessPointListener"
)


@dataclass
class _AccessPointControllerState:
    proxy: f_wlan_policy.AccessPointControllerClient
    updates: asyncio.Queue[list[AccessPointState]]
    # Keep the async task for fuchsia.wlan.policy/AccessPointStateUpdates so it
    # doesn't get garbage collected when cancelled.
    access_point_state_updates_server_task: asyncio.Task[None]


class WlanPolicyAp(AsyncLazyReady):
    """WlanPolicyAp affordance implemented with Fuchsia Controller."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
        fuchsia_device_close: affordances_capable.FuchsiaDeviceClose,
    ) -> None:
        """Create an WlanPolicyAp Fuchsia Controller affordance.

        Args:
            device_name: Device name returned by `ffx target list`.
            ffx: FFX transport.
            fuchsia_controller: Fuchsia Controller transport.
            reboot_affordance: Object that implements RebootCapableDevice.
            fuchsia_device_close: Object that implements FuchsiaDeviceClose.
        """
        AsyncLazyReady.__init__(self)

        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx
        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self._fuchsia_device_close = fuchsia_device_close

        self._access_point_controller: _AccessPointControllerState | None = None

        self.verify_supported()

        self._reboot_affordance.register_for_on_device_boot(self.make_ready)

    async def make_ready(self) -> None:
        await super().make_ready()
        device_monitor_proxy = f_wlan_device_service.DeviceMonitorClient(
            self._fc_transport.connect_device_proxy(
                FidlEndpoint(
                    "core/wlandevicemonitor",
                    "fuchsia.wlan.device.service.DeviceMonitor",
                )
            )
        )
        try:
            phy_list = (await device_monitor_proxy.list_phys()).phy_list

            phy_supported_roles = [
                (
                    await device_monitor_proxy.get_supported_mac_roles(
                        phy_id=phy_id
                    )
                )
                .unwrap()
                .supported_mac_roles
                for phy_id in phy_list
            ]
        except (AssertionError, ZxStatus, FcTransportStatus) as e:
            raise wlan_errors.HoneydewWlanError(
                "DeviceMonitor.GetSupportedMacRoles() error"
            ) from e
        if not any(
            [
                f_wlan_common.WlanMacRole.AP in roles
                for roles in phy_supported_roles
            ]
        ):
            raise wlan_errors.HoneydewWlanError(
                "Device does not support an access point interface."
            )

        await self._connect_proxy()

    def verify_supported(self) -> None:
        """Verifies that the WlanPolicyAp affordance using FuchsiaController is supported by the
        Fuchsia device.

        This method should be called in `__init__()` so that if this affordance was called on a
        Fuchsia device that does not support it, it will raise NotSupportedError.

        Raises:
            NotSupportedError: If affordance is not supported.
        """
        for capability in _REQUIRED_CAPABILITIES:
            # TODO(http://b/359342196): This is a maintenance burden; find a
            # better way to detect FIDL component capabilities.
            if capability not in self._ffx.run(
                ["component", "capability", capability],
                # TODO(b/474143046) update to JSON when ffx supports it
                machine=ffx_types.MachineFormat.RAW,
            ):
                _LOGGER.warning(
                    "All available WLAN component capabilities:\n%s",
                    self._ffx.run(
                        ["component", "capability", "fuchsia.wlan"],
                        # TODO(b/474143046) update to JSON when ffx supports it
                        machine=ffx_types.MachineFormat.RAW,
                    ),
                )
                raise errors.NotSupportedError(
                    f'Component capability "{capability}" not exposed by device '
                    f"{self._device_name}; this build of Fuchsia does not support the "
                    "WLAN FC affordance."
                )

    async def _connect_proxy(self) -> None:
        """Re-initializes connection to the WLAN stack.

        See fuchsia.wlan.policy/AccessPointProvider.GetController().

        Raises:
            HoneydewWlanError: Error from WLAN stack.
        """
        (
            controller_client,
            controller_server,
        ) = self._fc_transport.channel_create()
        access_point_controller_proxy = (
            f_wlan_policy.AccessPointControllerClient(controller_client.take())
        )

        updates: asyncio.Queue[list[AccessPointState]] = asyncio.Queue()

        updates_client, updates_server = self._fc_transport.channel_create()
        access_point_state_updates_server = AccessPointStateUpdatesImpl(
            updates_server, updates
        )
        task = asyncio.create_task(access_point_state_updates_server.serve())

        access_point_provider_proxy = f_wlan_policy.AccessPointProviderClient(
            self._fc_transport.connect_device_proxy(
                _ACCESS_POINT_PROVIDER_PROXY
            )
        )

        try:
            access_point_provider_proxy.get_controller(
                requests=controller_server.take(),
                updates=updates_client.take(),
            )
        except FcTransportStatus as status:
            raise wlan_errors.HoneydewWlanError(
                f"AccessPointProvider.GetController() error {status}"
            ) from status

        self._access_point_controller = _AccessPointControllerState(
            proxy=access_point_controller_proxy,
            updates=updates,
            access_point_state_updates_server_task=task,
        )

    @ensure_ready
    async def start(
        self,
        ssid: str,
        security: f_wlan_policy.SecurityType,
        password: str | None,
        mode: f_wlan_policy.ConnectivityMode,
        band: f_wlan_policy.OperatingBand,
    ) -> None:
        """Start an access point.

        Args:
            ssid: SSID of the network to start.
            security: The security protocol of the network.
            password: Credential used to connect to the network. None is
                equivalent to no password.
            mode: The connectivity mode to use
            band: The operating band to use

        Raises:
            HoneydewWlanError: Error from WLAN stack
            HoneydewWlanRequestRejectedError: WLAN rejected the request
        """
        assert self._access_point_controller is not None
        cred = Credential.from_password(password)

        try:
            resp = await self._access_point_controller.proxy.start_access_point(
                config=NetworkConfig(
                    ssid, security, cred.type(), cred.value()
                ).to_fidl(),
                mode=mode,
                band=band,
            )
        except FcTransportStatus as status:
            raise wlan_errors.HoneydewWlanError(
                f"AccessPointController.StartAccessPoint() error {status}"
            ) from status

        request_status = f_wlan_policy.RequestStatus(resp.status)
        if request_status is not f_wlan_policy.RequestStatus.ACKNOWLEDGED:
            raise wlan_errors.HoneydewWlanRequestRejectedError(
                "AccessPointController.StartAccessPoint()",
                request_status,
            )

    @ensure_ready
    async def stop(
        self,
        ssid: str,
        security: f_wlan_policy.SecurityType,
        password: str | None,
    ) -> None:
        """Stop an active access point.

        Args:
            ssid: SSID of the network to stop.
            security: The security protocol of the network.
            password: Credential used to connect to the network. None is
                equivalent to no password.

        Raises:
            HoneydewWlanError: Error from WLAN stack
            HoneydewWlanRequestRejectedError: WLAN rejected the request
        """
        assert self._access_point_controller is not None
        cred = Credential.from_password(password)

        try:
            resp = await self._access_point_controller.proxy.stop_access_point(
                config=NetworkConfig(
                    ssid, security, cred.type(), cred.value()
                ).to_fidl(),
            )
        except FcTransportStatus as status:
            raise wlan_errors.HoneydewWlanError(
                f"AccessPointController.StopAccessPoint() error {status}"
            ) from status

        request_status = f_wlan_policy.RequestStatus(resp.status)
        if request_status is not f_wlan_policy.RequestStatus.ACKNOWLEDGED:
            raise wlan_errors.HoneydewWlanRequestRejectedError(
                "AccessPointController.StopAccessPoint()",
                request_status,
            )

    @ensure_ready
    async def stop_all(self) -> None:
        """Stop all active access points.

        Raises:
            HoneydewWlanError: Error from WLAN stack
        """
        if self._access_point_controller:
            self._access_point_controller.proxy.stop_all_access_points()

    @ensure_ready
    async def set_new_update_listener(self) -> None:
        """Sets the update listener stream of the facade to a new stream.

        This causes updates to be reset. Intended to be used between tests so
        that the behavior of updates in a test is independent from previous
        tests.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
        """
        # Replace the existing AccessPointStateUpdates server without giving up our
        # handle to AccessPointController. This is necessary since the AccessPointProvider
        # API is designed to only allow a single caller to make AccessPointController
        # calls which would impact WLAN state. If we lose our handle to the
        # AccessPointController, some other component on the system could take it.
        assert self._access_point_controller is not None
        self._access_point_controller.access_point_state_updates_server_task.cancel()
        try:
            await self._access_point_controller.access_point_state_updates_server_task
        except asyncio.exceptions.CancelledError:
            pass

        access_point_listener_proxy = f_wlan_policy.AccessPointListenerClient(
            self._fc_transport.connect_device_proxy(
                _ACCESS_POINT_LISTENER_PROXY
            )
        )

        updates: asyncio.Queue[list[AccessPointState]] = asyncio.Queue()
        updates_client, updates_server = self._fc_transport.channel_create()
        access_point_state_updates_server = AccessPointStateUpdatesImpl(
            updates_server, updates
        )
        task = asyncio.create_task(access_point_state_updates_server.serve())

        try:
            access_point_listener_proxy.get_listener(
                updates=updates_client.take(),
            )
        except FcTransportStatus as status:
            raise wlan_errors.HoneydewWlanError(
                f"AccessPointListener.GetListener() error {status}"
            ) from status

        self._access_point_controller.updates = updates
        self._access_point_controller.access_point_state_updates_server_task = (
            task
        )

    @ensure_ready
    async def get_update(
        self,
        timeout: float | None = None,
    ) -> list[AccessPointState]:
        """Get a list of AP state listener updates.

        This call will return with an update immediately the
        first time the update listener is initialized by setting a new listener
        or by creating a client controller before setting a new listener.
        Subsequent calls will hang until there is a change since the last
        update call.

        Args:
            timeout: Timeout in seconds to wait for the get_update command to
                return. By default it is set to None (which means timeout is
                disabled)

        Returns:
            A list of AP state updates.

        Raises:
            HoneydewWlanError: Error from WLAN stack.
            TimeoutError: Reached timeout without any updates.
        """
        assert self._access_point_controller is not None
        return await asyncio.wait_for(
            self._access_point_controller.updates.get(), timeout
        )


class AccessPointStateUpdatesImpl(f_wlan_policy.AccessPointStateUpdatesServer):
    """Server to receive WLAN access point state changes.

    Receive updates on the current summary of wlan access point operating
    states. This will be called when there are changes with active access point
    networks - both the number of access points and their individual activity.
    """

    def __init__(
        self, server: Channel, updates: asyncio.Queue[list[AccessPointState]]
    ) -> None:
        super().__init__(server)
        self._updates = updates
        _LOGGER.debug("Started AccessPointStateUpdates server")

    async def on_access_point_state_update(
        self,
        request: f_wlan_policy.AccessPointStateUpdatesOnAccessPointStateUpdateRequest,
    ) -> None:
        """Detected a change to the state or registered listeners.

        Args:
            request: Current summary of WLAN access point operating states.
        """
        access_points = [
            AccessPointState.from_fidl(ap) for ap in request.access_points
        ]
        _LOGGER.debug(
            "OnAccessPointStateUpdates called with %s", repr(access_points)
        )
        await self._updates.put(access_points)
