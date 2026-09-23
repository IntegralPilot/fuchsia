# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import asyncio
import logging
from collections.abc import Sequence
from dataclasses import dataclass

import fidl_fuchsia_wlan_common as f_wlan_common
import fidl_fuchsia_wlan_device_service as f_wlan_device_service
import fidl_fuchsia_wlan_ieee80211 as f_wlan_ieee80211
import fidl_fuchsia_wlan_internal as f_wlan_internal
import fidl_fuchsia_wlan_sme as f_wlan_sme
import fidl_fuchsia_wlan_stats as f_wlan_stats
from fidl._client import FidlClient
from fuchsia_controller_py import FcTransportStatus, ZxStatus
from honeydew import affordances_capable
from honeydew.affordances.affordance import AsyncLazyReady, ensure_ready
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import (
    BssDescriptionParser,
    CountryCode,
)
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing.custom_types import FidlEndpoint, MacAddress

logger = logging.getLogger(__name__)


class ClientIface:
    def __init__(
        self,
        iface_id: int,
        device_monitor: f_wlan_device_service.DeviceMonitorClient,
        client_sme: f_wlan_sme.ClientSmeClient,
        fuchsia_controller: fc_transport.FuchsiaController,
    ) -> None:
        self.id: int = iface_id
        self.role: f_wlan_common.WlanMacRole = f_wlan_common.WlanMacRole.CLIENT
        self._device_monitor: f_wlan_device_service.DeviceMonitorClient = (
            device_monitor
        )
        self.client_sme: f_wlan_sme.ClientSmeClient = client_sme
        self._fc_transport: fc_transport.FuchsiaController = fuchsia_controller

    @classmethod
    async def from_id(cls, iface_id: int, phy: Phy) -> ClientIface:
        client, server = phy._fc_transport.channel_create()
        client_sme = f_wlan_sme.ClientSmeClient(client)
        await phy.device_monitor.get_client_sme(
            iface_id=iface_id, sme_server=server.take()
        )
        return cls(
            iface_id=iface_id,
            device_monitor=phy.device_monitor,
            client_sme=client_sme,
            fuchsia_controller=phy._fc_transport,
        )

    async def connect(
        self,
        ssid: str,
        bss_desc: f_wlan_ieee80211.BssDescription,
        authentication: f_wlan_internal.Authentication,
    ) -> None:
        client, server = self._fc_transport.channel_create()
        connect_transaction_client = f_wlan_sme.ConnectTransactionClient(
            client.take()
        )

        req = f_wlan_sme.ConnectRequest(
            ssid=list(ssid.encode("utf-8")),
            bss_description=bss_desc,
            multiple_bss_candidates=False,
            authentication=authentication,
            deprecated_scan_type=f_wlan_common.ScanType.ACTIVE,
        )

        event_handler = ConnectTransactionEventHandler(
            connect_transaction_client
        )
        event_handler_task = asyncio.create_task(event_handler.serve())

        try:
            self.client_sme.connect(req=req, txn=server.take())

            on_connect_result_req = await asyncio.wait_for(
                event_handler.txn_queue.get(), timeout=60
            )
            assert isinstance(
                on_connect_result_req,
                f_wlan_sme.ConnectTransactionOnConnectResultRequest,
            )
            result = on_connect_result_req.result
        except TimeoutError as e:
            raise HoneydewWlanError(
                f'Timed out waiting for the WLAN SME to connect to "{ssid}"'
            ) from e
        finally:
            event_handler_task.cancel()
            try:
                await event_handler_task
            except asyncio.exceptions.CancelledError:
                pass

        if result.code != f_wlan_ieee80211.StatusCode.SUCCESS:
            code = f_wlan_ieee80211.StatusCode(result.code)
            raise HoneydewWlanError(
                f'Failed to connect to "{ssid}", received {code.name}({code.value})'
            )
        if result.is_credential_rejected:
            raise HoneydewWlanError(
                f'Failed to connect to "{ssid}", credentials were rejected.'
            )

        client_status = await self.status()
        if client_status.connected:
            got_ssid = bytes(client_status.connected.ssid).decode("utf-8")
            if got_ssid != ssid:
                raise HoneydewWlanError(
                    f'Connected to wrong network. Expected "{ssid}", got "{got_ssid}".'
                )
            if (
                client_status.connected.primary.number
                != bss_desc.primary.number
            ):
                raise HoneydewWlanError(
                    f"Connected to wrong channel. Expected channel {bss_desc.primary.number}, "
                    f"got {client_status.connected.primary.number}."
                )
        else:
            raise HoneydewWlanError(
                f"Expected ClientStatusResponse.connected, got {client_status}"
            )

    async def scan_and_connect(
        self,
        ssid: str,
        password: str | None = None,
        security: f_wlan_internal.Protocol = f_wlan_internal.Protocol.OPEN,
    ) -> None:
        try:
            scan_results = await self.passive_scan()
            target_bss_desc_list = scan_results[ssid]
            assert (
                len(target_bss_desc_list) > 0
            ), f"Scan result list for SSID {ssid} exists, but it's empty."
            if len(target_bss_desc_list) > 1:
                logger.warn(
                    "Choosing first scan result for SSID %s: %s",
                    ssid,
                    target_bss_desc_list,
                )
            target_bss_desc = target_bss_desc_list[0]
        except KeyError as e:
            raise HoneydewWlanError(
                f"Failed to find a BSS with SSID {ssid}: {scan_results}"
            ) from e

        if security == f_wlan_internal.Protocol.OPEN:
            assert (
                password is None
            ), "Password provided with Open security mode."
            credentials = None
        elif security == f_wlan_internal.Protocol.WEP:
            assert (
                password is not None
            ), "Password NOT provided WEP security mode."
            credentials = f_wlan_internal.Credentials(
                wep=f_wlan_internal.WepCredentials(
                    key=list(password.encode("utf-8"))
                )
            )
        else:
            assert (
                password is not None
            ), "Password NOT provided for implied WPA* security mode."
            credentials = f_wlan_internal.Credentials(
                wpa=f_wlan_internal.WpaCredentials(
                    passphrase=list(password.encode("utf-8"))
                )
            )

        authentication = f_wlan_internal.Authentication(
            protocol=security,
            credentials=credentials,
        )

        await self.connect(
            ssid=ssid, bss_desc=target_bss_desc, authentication=authentication
        )

    async def disconnect(self) -> None:
        await self.client_sme.disconnect(
            reason=f_wlan_sme.UserDisconnectReason.WLAN_SERVICE_UTIL_TESTING,
        )

    async def status(self) -> f_wlan_sme.ClientStatusResponse:
        return (await self.client_sme.status()).resp

    async def passive_scan(
        self,
        channels: Sequence[int] | None = None,
    ) -> dict[str, list[f_wlan_ieee80211.BssDescription]]:
        if channels is None:
            channels = []

        req = f_wlan_sme.ScanRequest(
            passive=f_wlan_sme.PassiveScanRequest(channels=channels)
        )

        scan_for_controller_response = (
            await self.client_sme.scan_for_controller(req=req)
        ).unwrap()

        results: dict[str, list[f_wlan_ieee80211.BssDescription]] = {}

        for scan_result in scan_for_controller_response.scan_results:
            desc = scan_result.bss_description
            ssid = BssDescriptionParser.ssid(desc)
            if ssid:
                if ssid in results:
                    results[ssid].append(desc)
                else:
                    results[ssid] = [desc]
            else:
                logger.warning(
                    "Scan result does not contain SSID: %s", scan_result
                )

        return results

    async def query(self) -> f_wlan_device_service.QueryIfaceResponse:
        return (
            (await self._device_monitor.query_iface(iface_id=self.id))
            .unwrap()
            .resp
        )

    async def get_mac_address(self) -> MacAddress:
        return MacAddress(bytes((await self.query()).sta_addr))

    async def get_signal_report(
        self,
    ) -> f_wlan_stats.SignalReport:
        client, server = self._fc_transport.channel_create()
        telemetry_client = f_wlan_sme.TelemetryClient(client)
        res = await self._device_monitor.get_sme_telemetry(
            iface_id=self.id, telemetry_server=server.take()
        )
        res.unwrap()
        return (await telemetry_client.get_signal_report()).unwrap().stats

    async def destroy(self) -> None:
        req = f_wlan_device_service.DestroyIfaceRequest(iface_id=self.id)
        destroy_iface = await self._device_monitor.destroy_iface(req=req)
        if destroy_iface.status != ZxStatus.ZX_OK:
            raise ZxStatus(destroy_iface.status)


class ApIface:
    def __init__(
        self,
        iface_id: int,
        device_monitor: f_wlan_device_service.DeviceMonitorClient,
        ap_sme: f_wlan_sme.ApSmeClient | None,
    ) -> None:
        self.id: int = iface_id
        self.role: f_wlan_common.WlanMacRole = f_wlan_common.WlanMacRole.AP
        self._device_monitor: f_wlan_device_service.DeviceMonitorClient = (
            device_monitor
        )
        self.ap_sme: f_wlan_sme.ApSmeClient | None = ap_sme

    @classmethod
    async def from_id(cls, iface_id: int, phy: Phy) -> ApIface:
        client, server = phy._fc_transport.channel_create()
        ap_sme: f_wlan_sme.ApSmeClient | None = None
        try:
            await phy.device_monitor.get_ap_sme(
                iface_id=iface_id, sme_server=server.take()
            )
            ap_sme = f_wlan_sme.ApSmeClient(client)
        except (FcTransportStatus, ZxStatus, AssertionError) as e:
            logger.warning(
                "Could not get ApSmeClient for iface %s: %s", iface_id, e
            )
        return cls(
            iface_id=iface_id,
            device_monitor=phy.device_monitor,
            ap_sme=ap_sme,
        )

    async def query(self) -> f_wlan_device_service.QueryIfaceResponse:
        return (
            (await self._device_monitor.query_iface(iface_id=self.id))
            .unwrap()
            .resp
        )

    async def get_mac_address(self) -> MacAddress:
        return MacAddress(bytes((await self.query()).sta_addr))

    async def destroy(self) -> None:
        req = f_wlan_device_service.DestroyIfaceRequest(iface_id=self.id)
        destroy_iface = await self._device_monitor.destroy_iface(req=req)
        if destroy_iface.status != ZxStatus.ZX_OK:
            raise ZxStatus(destroy_iface.status)


class Phy:
    def __init__(
        self,
        phy_id: int,
        device_monitor: f_wlan_device_service.DeviceMonitorClient,
        fuchsia_controller: fc_transport.FuchsiaController,
    ) -> None:
        self.id: int = phy_id
        self.device_monitor: f_wlan_device_service.DeviceMonitorClient = (
            device_monitor
        )
        self._fc_transport: fc_transport.FuchsiaController = fuchsia_controller

    async def create_client_iface(
        self, sta_address: MacAddress | None = None
    ) -> ClientIface:
        if sta_address is None:
            sta_address = MacAddress("00:00:00:00:00:00")
            logger.warning(
                "No MAC provided in args of create_client_iface, using %s",
                sta_address,
            )

        create_iface_response = (
            await self.device_monitor.create_iface(
                phy_id=self.id,
                role=f_wlan_common.WlanMacRole.CLIENT,
                sta_address=bytes(sta_address),
            )
        ).unwrap()

        assert (
            create_iface_response.iface_id is not None
        ), f"{create_iface_response!r} missing iface_id"
        return await ClientIface.from_id(create_iface_response.iface_id, self)

    async def create_ap_iface(
        self, sta_address: MacAddress | None = None
    ) -> ApIface:
        if sta_address is None:
            sta_address = MacAddress("00:00:00:00:00:00")
            logger.warning(
                "No MAC provided in args of create_ap_iface, using %s",
                sta_address,
            )

        create_iface_response = (
            await self.device_monitor.create_iface(
                phy_id=self.id,
                role=f_wlan_common.WlanMacRole.AP,
                sta_address=bytes(sta_address),
            )
        ).unwrap()

        assert (
            create_iface_response.iface_id is not None
        ), f"{create_iface_response!r} missing iface_id"
        return await ApIface.from_id(create_iface_response.iface_id, self)

    async def get_ifaces(self) -> list[ClientIface | ApIface]:
        """Returns all interfaces associated with this PHY."""
        list_ifaces_response = await self.device_monitor.list_ifaces()
        ifaces: list[ClientIface | ApIface] = []
        for iface_id in list_ifaces_response.iface_list:
            try:
                query_response = (
                    await self.device_monitor.query_iface(iface_id=iface_id)
                ).unwrap()
            except (FcTransportStatus, ZxStatus):
                continue
            if (
                query_response.resp is None
                or query_response.resp.phy_id != self.id
            ):
                continue
            if query_response.resp.role == f_wlan_common.WlanMacRole.CLIENT:
                ifaces.append(await ClientIface.from_id(iface_id, self))
            elif query_response.resp.role == f_wlan_common.WlanMacRole.AP:
                ifaces.append(await ApIface.from_id(iface_id, self))
            else:
                logger.warning(
                    "Encountered iface %s with unsupported role %s on phy %s",
                    iface_id,
                    query_response.resp.role,
                    self.id,
                )
        return ifaces

    async def get_client_ifaces(self) -> list[ClientIface]:
        """Returns all client interfaces associated with this PHY."""
        return [
            iface
            for iface in await self.get_ifaces()
            if isinstance(iface, ClientIface)
        ]

    async def get_ap_ifaces(self) -> list[ApIface]:
        """Returns all AP interfaces associated with this PHY."""
        return [
            iface
            for iface in await self.get_ifaces()
            if isinstance(iface, ApIface)
        ]

    async def get_country(self) -> CountryCode:
        return CountryCode(
            bytes(
                (await self.device_monitor.get_country(phy_id=self.id))
                .unwrap()
                .resp.alpha2
            )
        )

    async def set_country(self, country_code: CountryCode) -> None:
        response = await self.device_monitor.set_country(
            req=f_wlan_device_service.SetCountryRequest(
                phy_id=self.id,
                alpha2=bytes(country_code),
            )
        )
        if response.status != ZxStatus.ZX_OK:
            raise ZxStatus(response.status)


class WlanCore(AsyncLazyReady):
    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        fuchsia_controller: fc_transport.FuchsiaController,
        reboot_affordance: affordances_capable.RebootCapableDevice,
        fuchsia_device_close: affordances_capable.FuchsiaDeviceClose,
    ) -> None:
        AsyncLazyReady.__init__(self)

        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx
        self._fc_transport = fuchsia_controller
        self._reboot_affordance = reboot_affordance
        self._fuchsia_device_close = fuchsia_device_close

        self._reboot_affordance.register_for_on_device_boot(self.make_ready)

    async def make_ready(self) -> None:
        self._device_monitor_proxy = f_wlan_device_service.DeviceMonitorClient(
            self._fc_transport.connect_device_proxy(
                FidlEndpoint(
                    "core/wlandevicemonitor",
                    "fuchsia.wlan.device.service.DeviceMonitor",
                )
            )
        )
        await super().make_ready()

    @ensure_ready
    async def destroy_all_ifaces(self) -> None:
        ifaces = (await self._device_monitor_proxy.list_ifaces()).iface_list
        for iface_id in ifaces:
            logger.info("Destroying WLAN interface %s", iface_id)
            req = f_wlan_device_service.DestroyIfaceRequest(iface_id=iface_id)
            destroy_iface = await self._device_monitor_proxy.destroy_iface(
                req=req
            )
            if destroy_iface.status != ZxStatus.ZX_OK:
                raise ZxStatus(destroy_iface.status)

    @ensure_ready
    async def ensure_single_phy(self) -> Phy:
        """Asserts there is only one PHY device and returns its Phy object."""
        proxy, server = self._fc_transport.channel_create()
        device_watcher_client = f_wlan_device_service.DeviceWatcherClient(
            proxy.take()
        )
        self._device_monitor_proxy.watch_devices(watcher=server.take())

        phy_id = None
        async with DeviceWatcherEventHandler(device_watcher_client) as ctx:
            while True:
                try:
                    # E2E tests should run sufficiently long after
                    # boot that any additional PHY will already be in
                    # the queue, even if they bind some non-trivial
                    # time after the first PHY. In other words, a small
                    # non-zero timeout is sufficient.
                    next_txn = await asyncio.wait_for(
                        ctx.txn_queue.get(), timeout=0.1
                    )
                    if isinstance(
                        next_txn,
                        f_wlan_device_service.DeviceWatcherOnPhyAddedRequest,
                    ):
                        if phy_id is not None:
                            raise HoneydewWlanError(
                                f"Detected second PHY {next_txn.phy_id}! First PHY was {phy_id}."
                            )
                        phy_id = next_txn.phy_id
                    elif isinstance(
                        next_txn,
                        (
                            f_wlan_device_service.DeviceWatcherOnIfaceAddedRequest,
                            f_wlan_device_service.DeviceWatcherOnIfaceRemovedRequest,
                            f_wlan_device_service.DeviceWatcherOnPhyRemovedRequest,
                        ),
                    ):
                        logger.info(
                            "Ignoring DeviceWatcher event: %s",
                            next_txn,
                        )
                except TimeoutError:
                    logger.debug("Assuming all DeviceWatcher events observed.")
                    break

        if phy_id is None:
            raise HoneydewWlanError("DeviceWatcher failed to report a phy.")
        return Phy(
            phy_id=phy_id,
            device_monitor=self._device_monitor_proxy,
            fuchsia_controller=self._fc_transport,
        )


@dataclass
class ConnectTransactionContext:
    txn_queue: asyncio.Queue[
        f_wlan_sme.ConnectTransactionOnConnectResultRequest
        | f_wlan_sme.ConnectTransactionOnDisconnectRequest
        | f_wlan_sme.ConnectTransactionOnRoamResultRequest
        | f_wlan_sme.ConnectTransactionOnSignalReportRequest
        | f_wlan_sme.ConnectTransactionOnChannelSwitchedRequest
    ]


class ConnectTransactionEventHandler(f_wlan_sme.ConnectTransactionEventHandler):
    def __init__(
        self,
        client: FidlClient,
    ) -> None:
        super().__init__(client)
        self.txn_queue: asyncio.Queue[
            f_wlan_sme.ConnectTransactionOnConnectResultRequest
            | f_wlan_sme.ConnectTransactionOnDisconnectRequest
            | f_wlan_sme.ConnectTransactionOnRoamResultRequest
            | f_wlan_sme.ConnectTransactionOnSignalReportRequest
            | f_wlan_sme.ConnectTransactionOnChannelSwitchedRequest
        ] = asyncio.Queue()
        self.server_task: asyncio.Task[None] | None = None

    def on_connect_result(
        self, request: f_wlan_sme.ConnectTransactionOnConnectResultRequest
    ) -> None:
        logger.debug(
            "ConnectTransaction.OnConnectResult() called with %s", request
        )
        self.txn_queue.put_nowait(request)

    def on_disconnect(
        self, request: f_wlan_sme.ConnectTransactionOnDisconnectRequest
    ) -> None:
        logger.debug(
            "ConnectTransaction.OnDisconnect() called with %s", request
        )
        self.txn_queue.put_nowait(request)

    def on_roam_result(
        self, request: f_wlan_sme.ConnectTransactionOnRoamResultRequest
    ) -> None:
        logger.debug(
            "ConnectTransaction.OnRoamResult() called with %s", request
        )
        self.txn_queue.put_nowait(request)

    def on_signal_report(
        self, request: f_wlan_sme.ConnectTransactionOnSignalReportRequest
    ) -> None:
        logger.debug(
            "ConnectTransaction.OnSignalReport() called with %s", request
        )
        self.txn_queue.put_nowait(request)

    def on_channel_switched(
        self, request: f_wlan_sme.ConnectTransactionOnChannelSwitchedRequest
    ) -> None:
        logger.debug(
            "ConnectTransaction.OnChannelSwitched() called with %s", request
        )
        self.txn_queue.put_nowait(request)

    async def __aenter__(self) -> ConnectTransactionContext:
        self.server_task = asyncio.create_task(self.serve())
        return ConnectTransactionContext(txn_queue=self.txn_queue)

    async def __aexit__(self, *args: object) -> None:
        if self.server_task is not None:
            self.server_task.cancel()
            try:
                await self.server_task
            except asyncio.CancelledError:
                pass


@dataclass
class DeviceWatcherContext:
    txn_queue: asyncio.Queue[
        f_wlan_device_service.DeviceWatcherOnPhyAddedRequest
        | f_wlan_device_service.DeviceWatcherOnPhyRemovedRequest
        | f_wlan_device_service.DeviceWatcherOnIfaceAddedRequest
        | f_wlan_device_service.DeviceWatcherOnIfaceRemovedRequest
    ]


class DeviceWatcherEventHandler(
    f_wlan_device_service.DeviceWatcherEventHandler
):
    def __init__(
        self,
        client: FidlClient,
    ) -> None:
        self.client = client
        self.server_task: asyncio.Task[None] | None = None

    def on_phy_added(
        self, request: f_wlan_device_service.DeviceWatcherOnPhyAddedRequest
    ) -> None:
        logger.debug("DeviceWatcher.OnPhyAdded() called with %s", request)
        self.txn_queue.put_nowait(request)

    def on_phy_removed(
        self, request: f_wlan_device_service.DeviceWatcherOnPhyRemovedRequest
    ) -> None:
        logger.debug("DeviceWatcher.OnPhyRemoved() called with %s", request)
        self.txn_queue.put_nowait(request)

    def on_iface_added(
        self, request: f_wlan_device_service.DeviceWatcherOnIfaceAddedRequest
    ) -> None:
        logger.debug("DeviceWatcher.OnPhyAdded() called with %s", request)
        self.txn_queue.put_nowait(request)

    def on_iface_removed(
        self, request: f_wlan_device_service.DeviceWatcherOnIfaceRemovedRequest
    ) -> None:
        logger.debug("DeviceWatcher.OnIfaceRemoved() called with %s", request)
        self.txn_queue.put_nowait(request)

    async def __aenter__(self) -> DeviceWatcherContext:
        super().__init__(client=self.client)
        self.txn_queue: asyncio.Queue[
            f_wlan_device_service.DeviceWatcherOnPhyAddedRequest
            | f_wlan_device_service.DeviceWatcherOnPhyRemovedRequest
            | f_wlan_device_service.DeviceWatcherOnIfaceAddedRequest
            | f_wlan_device_service.DeviceWatcherOnIfaceRemovedRequest
        ] = asyncio.Queue()
        self.server_task = asyncio.create_task(self.serve())
        return DeviceWatcherContext(txn_queue=self.txn_queue)

    async def __aexit__(self, *args: object) -> None:
        if self.server_task is not None:
            self.server_task.cancel()
            try:
                await self.server_task
            except asyncio.CancelledError:
                pass
