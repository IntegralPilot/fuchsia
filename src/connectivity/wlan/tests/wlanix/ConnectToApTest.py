# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Tests for connecting to an access point.
"""

import logging

logger = logging.getLogger(__name__)

import asyncio
import struct
from dataclasses import dataclass
from typing import Any

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
import wlanix_testing.base_test as base_test
from antlion import utils
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib.hostapd_constants import (
    AP_DEFAULT_CHANNEL_2G,
    AP_SSID_LENGTH_2G,
)
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from fuchsia_controller_py import Channel
from mobly import signals, test_runner
from mobly.asserts import assert_equal, assert_true
from openwrt_access_point import OpenWrtAP
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    AccessPointConfig,
    BssSettings,
    RadioConfig,
    Security,
    SecurityOpen,
    SecurityWpa2,
    SecurityWpa3,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)


class ConnectToApTest(base_test.ConnectionBaseTestClass):
    async def pre_run(self) -> None:
        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self.name_func,
            arg_sets=[
                (SecurityOpen(), None),
                (SecurityWpa2(), AccessPointConfig.random_string()),
                (SecurityWpa3(), AccessPointConfig.random_string()),
            ],
        )

    def name_func(self, security: Security, password: str | None) -> str:
        return f"test_successfully_connect_to_ap_{security}"

    async def _test_logic(
        self, security: Security, password: str | None
    ) -> None:
        ssid = utils.rand_ascii_str(AP_SSID_LENGTH_2G)

        ap = self.access_point()
        if isinstance(ap, OpenWrtAP):
            ap.configure_wifi(
                AccessPointConfig(
                    radios=[
                        RadioConfig(
                            channel=DEFAULT_2G_CHANNEL,
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid,
                                    password=password,
                                    security=security,
                                )
                            ],
                        )
                    ]
                )
            )
        elif isinstance(ap, AccessPoint):
            setup_ap(
                access_point=ap,
                profile_name="whirlwind",
                channel=AP_DEFAULT_CHANNEL_2G,
                ssid=ssid,
                security=DeprecatedSecurity(
                    security_mode=ConfigMapper.to_hostapd_security(security),
                    password=password,
                ),
            )

        logger.info("Querying for IfaceIndex...")
        get_interface_message = fidl_wlanix.Nl80211Message(
            message=fidl_wlanix.Message(
                # fmt: off
                payload=[
                    # Generic Netlink Header
                    0x05,  # Command: GetInterface
                    0x01,  # Version
                    0x00, 0x00 # Reserved
                ],
                # fmt: on
            )
        )
        response_list = (
            (await self.nl80211_proxy.message(message=get_interface_message))
            .unwrap()
            .responses
        )
        assert response_list is not None
        attrs = base_test.verify_new_interface_response(response_list)
        iface_index = struct.unpack(
            "<I", attrs[base_test.NL80211_ATTR_IFINDEX]
        )[0]
        logger.info("Using IfaceIndex %d for connection test", iface_index)

        logger.info("Triggering a scan on IfaceIndex %d", iface_index)
        client, server = self.dut.fuchsia_controller.channel_create()
        async with Nl80211MulticastServer(client, server) as ctx:
            scan_queue = ctx.message_queue
            scan_callback_channel = ctx.callback_channel

            self.nl80211_proxy.get_multicast(
                group="scan", multicast=scan_callback_channel.take()
            )

            trigger_scan_message = fidl_wlanix.Nl80211Message(
                message=fidl_wlanix.Message(
                    # fmt: off
                    payload=[
                        # Generic Netlink Header
                        0x21,  # Command: TriggerScan
                        0x01,  # Version
                        0x00, 0x00,  # Reserved
                        0x08, 0x00,  # Length
                        0x03, 0x00,  # Type: IfaceIndex (little-endian)
                        *list(struct.pack("<I", iface_index)),
                    ],
                    # fmt: on
                )
            )
            response_list = (
                (await self.nl80211_proxy.message(message=trigger_scan_message))
                .unwrap()
                .responses
            )
            assert response_list is not None
            assert_equal(
                len(response_list),
                1,
                "Response from TriggerScan should contain a single ACK message.",
            )
            assert response_list[0].ack, "Response should have been an ACK."

            # Wait for a multicast message to indicate the scan has completed.
            try:
                scan_message = await asyncio.wait_for(
                    scan_queue.get(), timeout=20
                )
                logger.info("Recieved nl80211 scan result signal")
                assert (
                    scan_message.message
                ), "Received a non-message nl80211 message"
                assert (
                    scan_message.message.payload is not None
                ), "Received scan result indication without payload"
                assert_equal(
                    scan_message.message.payload[0],
                    34,  # Command: NewScanResults
                    "Received unexpected scan result",
                )
            except TimeoutError:
                raise signals.TestFailure(
                    "Did not receive a scan result within 20 seconds"
                )

        client, server = self.dut.fuchsia_controller.channel_create()
        async with SupplicantStaIfaceCallbackServer(client, server) as ctx:
            state_change_queue = ctx.state_change_queue
            callback_channel = ctx.callback_channel

            self.supplicant_sta_iface_proxy.register_callback(
                callback=callback_channel.take()
            )

            (
                proxy,
                server,
            ) = self.dut.fuchsia_controller.channel_create()
            self.supplicant_sta_iface_proxy.add_network(network=server.take())
            supplicant_sta_network_proxy = (
                fidl_wlanix.SupplicantStaNetworkClient(proxy)
            )

            supplicant_sta_network_proxy.set_ssid(
                ssid=list(ssid.encode("ascii"))
            )
            if password:
                if isinstance(security, SecurityWpa3):
                    supplicant_sta_network_proxy.set_sae_password(
                        password=list(password.encode("ascii"))
                    )
                else:
                    supplicant_sta_network_proxy.set_psk_passphrase(
                        passphrase=list(password.encode("ascii"))
                    )

            try:
                (await supplicant_sta_network_proxy.select()).unwrap()
            except AssertionError as e:
                raise signals.TestFailure(
                    f'Failed to connect to "{ssid}" with {security}'
                ) from e

            state_change = await state_change_queue.get()
            assert isinstance(
                state_change,
                fidl_wlanix.SupplicantStaIfaceCallbackOnStateChangedRequest,
            ), f"Expected OnStateChanged. Got {state_change!r}"
            assert_equal(
                state_change.new_state,
                fidl_wlanix.StaIfaceCallbackState.COMPLETED,
            )
            assert_true(
                state_change_queue.empty(),
                "Unexpectedly received additional callback messages.",
            )
            logger.info(f'Successfully connected to "{ssid}"!')


@dataclass
class SupplicantStaIfaceCallbackContext:
    state_change_queue: asyncio.Queue[
        fidl_wlanix.SupplicantStaIfaceCallbackOnStateChangedRequest
        | fidl_wlanix.SupplicantStaIfaceCallbackOnDisconnectedRequest
        | fidl_wlanix.SupplicantStaIfaceCallbackOnAssociationRejectedRequest
    ]
    callback_channel: Channel


class SupplicantStaIfaceCallbackServer(
    fidl_wlanix.SupplicantStaIfaceCallbackServer
):
    def __init__(
        self,
        client: Channel,
        server: Channel,
        verbose: bool = True,
    ) -> None:
        # Defer initialization of parent class to __aenter__
        self.client = client
        self.server = server
        self.verbose = verbose

    def on_state_changed(
        self,
        request: fidl_wlanix.SupplicantStaIfaceCallbackOnStateChangedRequest,
    ) -> None:
        if self.verbose:
            logger.info("State changed: %s", request)
        self.state_change_queue.put_nowait(request)

    def on_disconnected(
        self,
        request: fidl_wlanix.SupplicantStaIfaceCallbackOnDisconnectedRequest,
    ) -> None:
        if self.verbose:
            logger.info("Disconnected: %s", request)
        self.state_change_queue.put_nowait(request)

    def on_association_rejected(
        self,
        request: fidl_wlanix.SupplicantStaIfaceCallbackOnAssociationRejectedRequest,
    ) -> None:
        if self.verbose:
            logger.info("Association rejected: %s", request)
        self.state_change_queue.put_nowait(request)

    async def __aenter__(self) -> SupplicantStaIfaceCallbackContext:
        super().__init__(channel=self.server)

        self.state_change_queue: asyncio.Queue[
            fidl_wlanix.SupplicantStaIfaceCallbackOnStateChangedRequest
            | fidl_wlanix.SupplicantStaIfaceCallbackOnDisconnectedRequest
            | fidl_wlanix.SupplicantStaIfaceCallbackOnAssociationRejectedRequest
        ] = asyncio.Queue()
        self.server_task = asyncio.create_task(self.serve())
        return SupplicantStaIfaceCallbackContext(
            state_change_queue=self.state_change_queue,
            callback_channel=self.client,
        )

    async def __aexit__(self, *args: Any, **kwargs: Any) -> None:
        if self.server_task:
            self.server_task.cancel()


@dataclass
class Nl80211MulticastServerContext:
    message_queue: asyncio.Queue[fidl_wlanix.Nl80211Message]
    callback_channel: Channel


class Nl80211MulticastServer(fidl_wlanix.Nl80211MulticastServer):
    def __init__(self, client: Channel, server: Channel) -> None:
        # Defer initialization of parent class to __aenter__
        self.client = client
        self.server = server

    async def message(
        self, request: fidl_wlanix.Nl80211MulticastMessageRequest
    ) -> None:
        if request.message is not None:
            self.message_queue.put_nowait(request.message)

    async def __aenter__(self) -> Nl80211MulticastServerContext:
        super().__init__(channel=self.server)
        self.message_queue: asyncio.Queue[
            fidl_wlanix.Nl80211Message
        ] = asyncio.Queue()
        self.server_task = asyncio.create_task(self.serve())
        return Nl80211MulticastServerContext(
            message_queue=self.message_queue,
            callback_channel=self.client,
        )

    async def __aexit__(self, *args: Any, **kwargs: Any) -> None:
        if self.server_task:
            self.server_task.cancel()


if __name__ == "__main__":
    test_runner.main()
