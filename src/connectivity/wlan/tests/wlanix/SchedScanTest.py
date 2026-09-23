# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Tests for starting a scheduled scan on an interface.
"""

import asyncio
import logging
import struct
import time
from dataclasses import dataclass
from datetime import timedelta
from typing import Any, Iterator

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
import wlanix_testing.base_test as base_test
from antlion import utils
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib.hostapd_constants import (
    AP_DEFAULT_CHANNEL_2G,
    AP_DEFAULT_CHANNEL_5G,
    AP_SSID_LENGTH_2G,
)
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.controllers.ap_lib.hostapd_security import (
    SecurityMode,
)
from common.utils.ies import read_ssid
from fuchsia_controller_py import Channel
from honeydew.utils.deadline import Deadline
from mobly import test_runner
from mobly.asserts import assert_equal, assert_true, fail
from openwrt_access_point import OpenWrtAP
from openwrt_access_point.lib.access_point_config import (
    DEFAULT_2G_CHANNEL,
    DEFAULT_5G_CHANNEL,
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    HtMode,
    RadioConfig,
    SecurityOpen,
)

logger = logging.getLogger(__name__)

NL80211_CMD_START_SCHED_SCAN = 0x4B
NL80211_CMD_STOP_SCHED_SCAN = 0x4C
NL80211_CMD_SCHED_SCAN_RESULTS = 77
NL80211_ATTR_IFINDEX = 0x03
NL80211_ATTR_SCAN_SSIDS = 45
NL80211_ATTR_SCAN_FREQUENCIES = 44
NL80211_ATTR_SCHED_SCAN_INTERVAL = 0x77
NL80211_ATTR_SCHED_SCAN_MATCH = 132
NL80211_CMD_GET_SCAN = 32
NL80211_ATTR_BSS = 47
NL80211_BSS_INFORMATION_ELEMENTS = 6
IE_TYPE_SSID = 0
NL80211_SCHED_SCAN_MATCH_ATTR_RSSI = 2
NL80211_ATTR_SCHED_SCAN_PLANS = 225
NL80211_SCHED_SCAN_PLAN_INTERVAL = 1
NL80211_SCHED_SCAN_PLAN_ITERATIONS = 2

DEFAULT_SCHED_SCAN_INTERVAL_MS = 5000
DEFAULT_MATCH_TIMEOUT_SEC = 15.0


def parse_netlink_attributes(
    payload: bytes, start_offset: int = 4
) -> dict[int, bytes]:
    """Parses Netlink attributes from a payload.

    Args:
        payload: The bytes to parse.
        start_offset: The offset to start parsing from (default 4 to skip GenNetlink header).

    Returns:
        A dictionary mapping attribute type to attribute value.
    """
    attrs = {}
    offset = start_offset
    while offset + 4 <= len(payload):
        nla_len, nla_type = struct.unpack_from("<HH", payload, offset)
        if nla_len < 4:
            break
        # The most significant bits are reserved for NLA_F_NESTED and NLA_F_NET_BYTEORDER.
        nla_type &= 0x3FFF
        value = payload[offset + 4 : offset + nla_len]
        attrs[nla_type] = value
        # Move offset forward to the next 4-byte boundary.
        offset += (nla_len + 3) & ~3
    return attrs


def build_nla(attr_type: int, value: bytes) -> bytes:
    true_len = 4 + len(value)
    header = struct.pack("<HH", true_len, attr_type)
    attr = header + value
    pad_len = (4 - (len(attr) % 4)) % 4
    return attr + b"\x00" * pad_len


def build_match_set(
    ssid: str | bytes,
    rssi: int | None = None,
    index: int = 1,
) -> bytes:
    """Builds an NLA structure for a single match set entry containing SSID and optional RSSI threshold."""
    ssid_bytes = ssid.encode("ascii") if isinstance(ssid, str) else ssid
    ssid_attr = build_nla(1, ssid_bytes)
    rssi_attr = (
        build_nla(NL80211_SCHED_SCAN_MATCH_ATTR_RSSI, struct.pack("<i", rssi))
        if rssi is not None
        else b""
    )
    return build_nla(index, ssid_attr + rssi_attr)


def build_start_sched_scan_payload(
    iface_index: int,
    match_attr: bytes | None = None,
    interval_ms: int | None = DEFAULT_SCHED_SCAN_INTERVAL_MS,
    scan_ssids_attr: bytes | None = None,
    freqs_attr: bytes | None = None,
    plans_attr: bytes | None = None,
) -> list[int]:
    """Constructs the NL80211_CMD_START_SCHED_SCAN payload list with specified attributes."""
    payload = [
        NL80211_CMD_START_SCHED_SCAN,  # Command
        0x01,  # Version
        0x00,
        0x00,  # Reserved
        *list(build_nla(NL80211_ATTR_IFINDEX, struct.pack("<I", iface_index))),
    ]
    if interval_ms is not None:
        payload.extend(
            build_nla(
                NL80211_ATTR_SCHED_SCAN_INTERVAL,
                struct.pack("<I", interval_ms),
            )
        )
    if match_attr is not None:
        payload.extend(match_attr)
    if scan_ssids_attr is not None:
        payload.extend(scan_ssids_attr)
    if freqs_attr is not None:
        payload.extend(freqs_attr)
    if plans_attr is not None:
        payload.extend(plans_attr)
    return payload


@dataclass
class Nl80211MulticastServerContext:
    message_queue: asyncio.Queue[fidl_wlanix.Nl80211Message]
    callback_channel: Channel


class Nl80211MulticastServer(fidl_wlanix.Nl80211MulticastServer):
    def __init__(self, client: Channel, server: Channel) -> None:
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


class SchedScanTest(base_test.ConnectionBaseTestClass):
    async def setup_test(self) -> None:
        await super().setup_test()
        await self._stop_sched_scan()

    async def teardown_test(self) -> None:
        await self._stop_sched_scan()
        await super().teardown_test()

    def _setup_ap(
        self,
        hidden: bool = False,
        preset_ssid: str | None = None,
        band: Any = Band.BAND_2G,
    ) -> str:
        ssid = (
            preset_ssid
            if preset_ssid
            else utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        )
        security = DeprecatedSecurity(security_mode=SecurityMode.OPEN)
        ap_channel = (
            AP_DEFAULT_CHANNEL_2G
            if band == Band.BAND_2G
            else AP_DEFAULT_CHANNEL_5G
        )

        ap = self.access_point()
        if isinstance(ap, OpenWrtAP):
            ap.configure_wifi(
                AccessPointConfig(
                    radios=[
                        RadioConfig(
                            channel=BssChannel(
                                band,
                                ap_channel,
                                HtMode(bw=20),
                            ),
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid,
                                    security=SecurityOpen(),
                                    hidden=hidden,
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
                channel=ap_channel,
                ssid=ssid,
                security=security,
                hidden=hidden,
            )
        logger.info(
            "Setup %s AP with SSID: %s", "hidden" if hidden else "visible", ssid
        )
        return ssid

    async def _stop_sched_scan(self) -> None:
        try:
            iface_index = await self._query_iface_index()
            payload = [
                NL80211_CMD_STOP_SCHED_SCAN,  # Command
                0x01,  # Version
                0x00,
                0x00,  # Reserved
                *list(
                    build_nla(
                        NL80211_ATTR_IFINDEX, struct.pack("<I", iface_index)
                    )
                ),
            ]
            stop_sched_scan_message = fidl_wlanix.Nl80211Message(
                message=fidl_wlanix.Message(payload=payload)
            )
            logger.info("Stopping any active scheduled scans")
            await self.nl80211_proxy.message(message=stop_sched_scan_message)
        except Exception as e:
            logger.warning(
                "Failed to stop scheduled scan during teardown/setup: %s", e
            )

    async def _query_iface_index(self) -> int:
        get_interface_message = fidl_wlanix.Nl80211Message(
            message=fidl_wlanix.Message(
                payload=[
                    0x05,  # Command: GetInterface
                    0x01,  # Version
                    0x00,
                    0x00,  # Reserved
                ],
            )
        )
        response_list = (
            (await self.nl80211_proxy.message(message=get_interface_message))
            .unwrap()
            .responses
        )
        assert response_list is not None
        attrs = base_test.verify_new_interface_response(response_list)
        return struct.unpack("<I", attrs[base_test.NL80211_ATTR_IFINDEX])[0]

    async def _trigger_and_verify_sched_scan(
        self,
        iface_index: int,
        payload: list[int],
        expected_ssids: list[str],
        timeout_seconds: int = 10,
    ) -> None:
        # Send command
        logger.info("Sending start scheduled scan netlink message")
        start_sched_scan_message = fidl_wlanix.Nl80211Message(
            message=fidl_wlanix.Message(payload=payload)
        )

        # Listen for multicast events
        client, server = self.dut.fuchsia_controller.channel_create()
        async with Nl80211MulticastServer(client, server) as ctx:
            scan_queue = ctx.message_queue
            scan_callback_channel = ctx.callback_channel

            self.nl80211_proxy.get_multicast(
                group="scan", multicast=scan_callback_channel.take()
            )

            # Send command
            response_list = (
                (
                    await self.nl80211_proxy.message(
                        message=start_sched_scan_message
                    )
                )
                .unwrap()
                .responses
            )

            # Verify acknowledgement
            assert response_list is not None
            assert_equal(
                len(response_list),
                1,
                "Response from start scheduled scan message should contain a single ack.",
            )
            assert_true(
                response_list[0].ack is not None,
                "Response from start scheduled scan message should have been an ack.",
            )
            logger.info("Received ack for start scheduled scan message")

            # Poll for scan results
            deadline = Deadline.from_timeout(timedelta(seconds=timeout_seconds))
            delay = 0.5
            expect_match = len(expected_ssids) > 0
            while not deadline.is_due():
                try:
                    msg = scan_queue.get_nowait()
                    if msg.message and msg.message.payload:
                        cmd = msg.message.payload[0]
                        if cmd == NL80211_CMD_SCHED_SCAN_RESULTS:
                            if not expect_match:
                                fail(
                                    "Received unexpected scheduled scan results message."
                                )

                            logger.info(
                                "Received scheduled scan results message."
                            )
                            # Send GET_SCAN to retrieve scan results
                            logger.info(
                                "Sending get scan netlink message to verify results"
                            )
                            get_scan_payload = [
                                NL80211_CMD_GET_SCAN,  # Command: GetScan
                                0x01,  # Version
                                0x00,
                                0x00,  # Reserved
                                *list(
                                    build_nla(
                                        NL80211_ATTR_IFINDEX,
                                        struct.pack("<I", iface_index),
                                    )
                                ),
                            ]
                            get_scan_message = fidl_wlanix.Nl80211Message(
                                message=fidl_wlanix.Message(
                                    payload=get_scan_payload
                                )
                            )
                            get_scan_response = (
                                (
                                    await self.nl80211_proxy.message(
                                        message=get_scan_message
                                    )
                                )
                                .unwrap()
                                .responses
                            )
                            assert get_scan_response is not None

                            def get_ssids_from_responses(
                                responses: Any,
                            ) -> Iterator[str]:
                                for resp in responses:
                                    if (
                                        resp.done
                                        or resp.message is None
                                        or resp.message.payload is None
                                    ):
                                        continue
                                    if resp.error is not None:
                                        logger.warning(
                                            f"Error in get scan response: {resp.error}"
                                        )
                                        continue

                                    resp_attrs = parse_netlink_attributes(
                                        bytes(resp.message.payload)
                                    )
                                    if NL80211_ATTR_BSS not in resp_attrs:
                                        continue

                                    bss_attrs = parse_netlink_attributes(
                                        resp_attrs[NL80211_ATTR_BSS],
                                        start_offset=0,
                                    )
                                    if (
                                        NL80211_BSS_INFORMATION_ELEMENTS
                                        not in bss_attrs
                                    ):
                                        continue

                                    ssid_str = read_ssid(
                                        bss_attrs[
                                            NL80211_BSS_INFORMATION_ELEMENTS
                                        ]
                                    )
                                    if ssid_str is not None:
                                        yield ssid_str

                            found_ssids = set(
                                get_ssids_from_responses(get_scan_response)
                            )
                            for expected_ssid in expected_ssids:
                                if expected_ssid in found_ssids:
                                    logger.info(
                                        f"Found expected SSID {expected_ssid} in scan results."
                                    )
                                else:
                                    fail(
                                        f"SSID {expected_ssid} not found in get scan response after scheduled scan results message."
                                    )
                            break
                except asyncio.QueueEmpty:
                    await asyncio.sleep(delay)
                    delay = min(delay * 2, 2.0)
            else:
                if expect_match:
                    fail("Timed out waiting for scheduled scan results message")
                else:
                    logger.info(
                        "Successfully timed out without receiving unexpected match."
                    )

    async def test_basic_sched_scan_2g(self) -> None:
        ssid = self._setup_ap(hidden=False)
        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH, build_match_set(ssid)
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[ssid],
        )

    async def test_basic_sched_scan_5g(self) -> None:
        ssid = self._setup_ap(hidden=False, band=Band.BAND_5G)
        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH, build_match_set(ssid)
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[ssid],
        )

    async def test_start_sched_scan_hidden_ssid(self) -> None:
        ssid = self._setup_ap(hidden=True)
        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH, build_match_set(ssid)
        )
        scan_ssids_attr = build_nla(
            NL80211_ATTR_SCAN_SSIDS, build_nla(1, ssid.encode("ascii"))
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
            scan_ssids_attr=scan_ssids_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[ssid],
        )

    async def test_start_sched_scan_with_frequency_filter_match(self) -> None:
        ssid = self._setup_ap(hidden=False)
        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH, build_match_set(ssid)
        )
        freq_nla = build_nla(1, struct.pack("<I", 2437))
        freqs_attr = build_nla(NL80211_ATTR_SCAN_FREQUENCIES, freq_nla)

        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
            freqs_attr=freqs_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[ssid],
        )

    async def test_start_sched_scan_with_frequency_filter_no_match(
        self,
    ) -> None:
        ssid = self._setup_ap(hidden=False)
        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH, build_match_set(ssid)
        )
        freq_nla = build_nla(1, struct.pack("<I", 2412))
        freqs_attr = build_nla(NL80211_ATTR_SCAN_FREQUENCIES, freq_nla)

        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
            freqs_attr=freqs_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[],
            timeout_seconds=12,
        )

    async def test_start_sched_scan_with_multiple_ssids(self) -> None:
        ssid_a = utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        ssid_b = utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        logger.info(f"Setting up APs with SSIDs: {ssid_a} and {ssid_b}")
        ap = self.access_point()
        if isinstance(ap, OpenWrtAP):
            # Setup a 2.4GHz and 5GHz AP.
            ap.configure_wifi(
                AccessPointConfig(
                    radios=[
                        RadioConfig(
                            channel=DEFAULT_2G_CHANNEL,
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid_a, security=SecurityOpen()
                                )
                            ],
                        ),
                        RadioConfig(
                            channel=DEFAULT_5G_CHANNEL,
                            bss_settings=[
                                BssSettings(
                                    ssid=ssid_b, security=SecurityOpen()
                                )
                            ],
                        ),
                    ]
                )
            )
        elif isinstance(ap, AccessPoint):
            # Whirlwind's setup_ap() manages hostapd per physical radio interface (wlan0 for 2.4GHz
            # and wlan1 for 5GHz). Calling setup_ap() with 2.4GHz twice overwrites /tmp/hostapd-wlan0.conf,
            # so we broadcast one SSID on 2.4GHz and the other on 5GHz to have both active simultaneously.
            setup_ap(
                access_point=ap,
                profile_name="whirlwind",
                channel=AP_DEFAULT_CHANNEL_2G,
                ssid=ssid_a,
                security=DeprecatedSecurity(security_mode=SecurityMode.OPEN),
            )
            setup_ap(
                access_point=ap,
                profile_name="whirlwind",
                channel=AP_DEFAULT_CHANNEL_5G,
                ssid=ssid_b,
                security=DeprecatedSecurity(security_mode=SecurityMode.OPEN),
            )

        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH,
            build_match_set(ssid_a, index=1) + build_match_set(ssid_b, index=2),
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[ssid_a, ssid_b],
        )

    async def test_start_sched_scan_with_rssi_threshold_match(self) -> None:
        ssid = self._setup_ap(hidden=False)
        iface_index = await self._query_iface_index()

        # Construct match attributes with a very permissive RSSI threshold (e.g. -100 dBm)
        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH,
            build_match_set(ssid, rssi=-100),
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[ssid],
        )

    async def test_start_sched_scan_with_rssi_threshold_no_match(self) -> None:
        ssid = self._setup_ap(hidden=False)
        iface_index = await self._query_iface_index()

        # Construct match attributes with a restrictive RSSI threshold (e.g. -10 dBm)
        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH,
            build_match_set(ssid, rssi=-10),
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
        )

        await self._trigger_and_verify_sched_scan(
            iface_index=iface_index,
            payload=payload,
            expected_ssids=[],
            timeout_seconds=12,
        )

    async def test_start_sched_scan_with_scan_plans(self) -> None:
        """Verify that scan plans execute at the requested multi-tiered intervals."""
        ssid = utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH, build_match_set(ssid)
        )

        plan_1_interval_sec = 3
        plan_1_iterations = 2
        plan_2_interval_sec = 10

        plan_1_interval = build_nla(
            NL80211_SCHED_SCAN_PLAN_INTERVAL,
            struct.pack("<I", plan_1_interval_sec),
        )
        plan_1_iter = build_nla(
            NL80211_SCHED_SCAN_PLAN_ITERATIONS,
            struct.pack("<I", plan_1_iterations),
        )
        plan_1 = build_nla(1, plan_1_interval + plan_1_iter)

        plan_2_interval = build_nla(
            NL80211_SCHED_SCAN_PLAN_INTERVAL,
            struct.pack("<I", plan_2_interval_sec),
        )
        plan_2_iter = build_nla(
            NL80211_SCHED_SCAN_PLAN_ITERATIONS, struct.pack("<I", 0)
        )
        plan_2 = build_nla(2, plan_2_interval + plan_2_iter)

        plans_attr = build_nla(NL80211_ATTR_SCHED_SCAN_PLANS, plan_1 + plan_2)

        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
            interval_ms=None,
            plans_attr=plans_attr,
        )

        client, server = self.dut.fuchsia_controller.channel_create()
        async with Nl80211MulticastServer(client, server) as ctx:
            scan_queue = ctx.message_queue
            self.nl80211_proxy.get_multicast(
                group="scan", multicast=ctx.callback_channel.take()
            )
            start_time = time.time()

            response_list = (
                (
                    await self.nl80211_proxy.message(
                        message=fidl_wlanix.Nl80211Message(
                            message=fidl_wlanix.Message(payload=payload)
                        )
                    )
                )
                .unwrap()
                .responses
            )
            assert_true(
                response_list and response_list[0].ack is not None,
                "Response should be an ack",
            )

            plan_1_duration_sec = plan_1_interval_sec * plan_1_iterations
            sleep_duration_sec = plan_1_duration_sec + 2
            logger.info(
                "Sent scheduled scan request. Ensuring AP is OFF for the first %d seconds.",
                sleep_duration_sec,
            )
            await asyncio.sleep(sleep_duration_sec)

            expected_discovery_time_sec = (
                plan_1_interval_sec * (plan_1_iterations - 1)
                + plan_2_interval_sec
            )
            logger.info(
                "Bringing UP the AP. If the firmware is now on the %d-second interval, it should discover it roughly ~%d seconds after the start time.",
                plan_2_interval_sec,
                expected_discovery_time_sec,
            )
            self._setup_ap(preset_ssid=ssid)

            event_received = False
            elapsed_at_match = 0.0
            while True:
                msg = await asyncio.wait_for(
                    scan_queue.get(), timeout=DEFAULT_MATCH_TIMEOUT_SEC
                )
                if msg.message and msg.message.payload:
                    cmd = msg.message.payload[0]
                    if cmd == NL80211_CMD_SCHED_SCAN_RESULTS:
                        event_received = True
                        elapsed_at_match = time.time() - start_time
                        break

            assert_true(event_received, "Failed to receive scan match event")
            logger.info(
                f"Match received at {elapsed_at_match:.2f} seconds after PNO start."
            )
            assert_true(
                11.5 < elapsed_at_match < 14.5,
                f"Elapsed time {elapsed_at_match:.2f}s implies the firmware didn't respect the multi-tier scan plans!",
            )

    async def test_pno_multiple_ssids_match(self) -> None:
        """Verify that discovering a newly matched SSID triggers a scheduled scan event."""
        ssid_a = utils.rand_ascii_str(AP_SSID_LENGTH_2G)
        ssid_b = utils.rand_ascii_str(AP_SSID_LENGTH_2G)

        # Setup SSID A
        logger.info(f"Setting up initial AP with SSID: {ssid_a}")
        self._setup_ap(preset_ssid=ssid_a)

        iface_index = await self._query_iface_index()

        match_attr = build_nla(
            NL80211_ATTR_SCHED_SCAN_MATCH,
            build_match_set(ssid_a, index=1) + build_match_set(ssid_b, index=2),
        )
        payload = build_start_sched_scan_payload(
            iface_index=iface_index,
            match_attr=match_attr,
            interval_ms=3000,
        )

        client, server = self.dut.fuchsia_controller.channel_create()
        async with Nl80211MulticastServer(client, server) as ctx:
            scan_queue = ctx.message_queue
            self.nl80211_proxy.get_multicast(
                group="scan", multicast=ctx.callback_channel.take()
            )

            # Send command
            response_list = (
                (
                    await self.nl80211_proxy.message(
                        message=fidl_wlanix.Nl80211Message(
                            message=fidl_wlanix.Message(payload=payload)
                        )
                    )
                )
                .unwrap()
                .responses
            )
            assert_true(
                response_list and response_list[0].ack is not None,
                "Response should be an ack",
            )

            logger.info("Waiting for first match event for SSID A...")
            event_a_received = False
            while True:
                msg = await asyncio.wait_for(
                    scan_queue.get(), timeout=DEFAULT_MATCH_TIMEOUT_SEC
                )
                if msg.message and msg.message.payload:
                    cmd = msg.message.payload[0]
                    if cmd == NL80211_CMD_SCHED_SCAN_RESULTS:
                        event_a_received = True
                        break

            assert_true(event_a_received, "Failed to receive first match event")

            # Setup AP with SSID B (replaces SSID A over the air)
            logger.info(
                f"Setting up AP with SSID B ({ssid_b}) to trigger second match event..."
            )
            self._setup_ap(preset_ssid=ssid_b)

            logger.info("Waiting for second match event for SSID B...")
            event_b_received = False
            while True:
                msg = await asyncio.wait_for(
                    scan_queue.get(), timeout=DEFAULT_MATCH_TIMEOUT_SEC
                )
                if msg.message and msg.message.payload:
                    cmd = msg.message.payload[0]
                    if cmd == NL80211_CMD_SCHED_SCAN_RESULTS:
                        event_b_received = True
                        break

            assert_true(
                event_b_received,
                "Failed to receive second match event after AP reconfiguration",
            )


if __name__ == "__main__":
    test_runner.main()
