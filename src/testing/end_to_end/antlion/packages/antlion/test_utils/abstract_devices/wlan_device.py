#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from datetime import timedelta
from typing import Any

import fuchsia_async_extension
from antlion.controllers import iperf_client
from antlion.controllers.ap_lib.hostapd_security import SecurityMode
from antlion.controllers.fuchsia_device import FuchsiaDevice
from antlion.controllers.iperf_client import IPerfClientBase
from antlion.controllers.pdu import PduDevice
from antlion.utils import PingResult
from honeydew.affordances.connectivity.netstack.errors import (
    HoneydewNetstackError,
)
from honeydew.affordances.connectivity.wlan import core as wlan_core
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from mobly.records import TestResultRecord

DEFAULT_ASSOCIATE_TIMEOUT = timedelta(seconds=30)


class FuchsiaWlanDevice:
    """Fuchsia device that supports WLAN."""

    def __init__(self, fuchsia_device: FuchsiaDevice):
        self.device = fuchsia_device
        self.device.configure_wlan()

    async def _get_client_iface(self) -> wlan_core.ClientIface:
        phy = await self.device.honeydew_fd.wlan_core.ensure_single_phy()
        client_ifaces = await phy.get_client_ifaces()
        if client_ifaces:
            return client_ifaces[0]
        return await phy.create_client_iface()

    @property
    def identifier(self) -> str:
        return self.device.ip

    @property
    def has_wep_support(self) -> bool:
        for line in self._get_wlandevicemonitor_config().splitlines():
            if "wep_supported" in line and "Bool(true)" in line:
                return True
        return False

    @property
    def has_wpa_support(self) -> bool:
        for line in self._get_wlandevicemonitor_config().splitlines():
            if "wpa1_supported" in line and "Bool(true)" in line:
                return True
        return False

    def _get_wlandevicemonitor_config(self) -> str:
        return self.device.ffx.run(
            ["component", "show", "core/wlandevicemonitor"]
        )

    def take_bug_report(self, _: TestResultRecord) -> None:
        self.device.take_bug_report()

    def associate(
        self,
        target_ssid: str,
        target_security: SecurityMode,
        target_pwd: str | None = None,
        key_mgmt: str | None = None,
        check_connectivity: bool = True,
        hidden: bool = False,
    ) -> bool:
        try:
            fuchsia_async_extension.get_loop().run_until_complete(
                self.device.honeydew_fd.wlan_policy.save_network(
                    target_ssid,
                    target_security.fuchsia_security_type(),
                    target_pwd=target_pwd,
                )
            )
            fuchsia_async_extension.get_loop().run_until_complete(
                self.device.honeydew_fd.wlan_policy.connect(
                    target_ssid,
                    target_security.fuchsia_security_type(),
                    timeout=DEFAULT_ASSOCIATE_TIMEOUT,
                )
            )
            return True
        except HoneydewWlanError as e:
            self.device.log.error(
                f"Failed to save and connect to {target_ssid} with "
                f"error: {e}"
            )
            return False

    def disconnect(self) -> None:
        """Function to disconnect from a Fuchsia WLAN device.
        Asserts if disconnect was not successful.
        """
        fuchsia_async_extension.get_loop().run_until_complete(
            self.device.honeydew_fd.wlan_policy.remove_all_networks()
        )
        fuchsia_async_extension.get_loop().run_until_complete(
            self.device.honeydew_fd.wlan_policy.wait_for_no_connections()
        )

    def ping(
        self,
        dest_ip: str,
        count: int = 3,
        interval: timedelta = timedelta(seconds=1),
        timeout: timedelta = timedelta(seconds=1),
        size: int = 25,
        additional_ping_params: str | None = None,
    ) -> PingResult:
        try:
            hd_result = fuchsia_async_extension.get_loop().run_until_complete(
                self.device.honeydew_fd.netstack.ping(
                    dest_ip,
                    count=count,
                    interval=interval,
                    timeout=timeout,
                    size=size,
                    additional_ping_params=additional_ping_params,
                )
            )
            return PingResult(
                raw_output=hd_result.raw_output,
                success=hd_result.any_pings_received,
                requested=count,
                transmitted=hd_result.transmitted,
                received=hd_result.received,
                time_ms=hd_result.time_ms,
                rtt_min_ms=hd_result.rtt_min_ms,
                rtt_avg_ms=hd_result.rtt_avg_ms,
                rtt_max_ms=hd_result.rtt_max_ms,
                rtt_mdev_ms=hd_result.rtt_mdev_ms,
            )
        except HoneydewNetstackError as e:
            return PingResult(
                raw_output=str(e),
                success=False,
                requested=count,
                transmitted=0,
                received=0,
                time_ms=None,
                rtt_min_ms=None,
                rtt_avg_ms=None,
                rtt_max_ms=None,
                rtt_mdev_ms=None,
            )

    def get_default_wlan_test_interface(self) -> str:
        if self.device.wlan_client_test_interface_name is None:
            raise TypeError(
                "Expected wlan_client_test_interface_name to be str"
            )
        return self.device.wlan_client_test_interface_name

    def is_connected(self, ssid: str | None = None) -> bool:
        async def _status() -> Any:
            iface = await self._get_client_iface()
            return await iface.status()

        result = fuchsia_async_extension.get_loop().run_until_complete(
            _status()
        )
        if result.idle:
            self.device.log.info("Client status idle")
            return False
        if result.connecting:
            ssid_bytes = bytearray(result.connecting).decode(
                encoding="utf-8", errors="replace"
            )
            self.device.log.info(
                f"Client status connecting to ssid: {ssid_bytes}"
            )
            return False
        if result.connected:
            ssid_bytes = bytearray(result.connected.ssid).decode(
                encoding="utf-8", errors="replace"
            )
            self.device.log.info(f"Client connected to ssid: {ssid_bytes}")
            if ssid is None:
                return True
            return ssid == ssid_bytes
        raise ValueError(
            f"Status did not return a valid status response: {result}"
        )

    def hard_power_cycle(self, pdus: list[PduDevice]) -> None:
        self.device.reboot(reboot_type="hard", testbed_pdus=pdus)

    def create_iperf_client(
        self, test_interface: str | None = None
    ) -> IPerfClientBase:
        if not test_interface:
            test_interface = self.get_default_wlan_test_interface()

        return iperf_client.IPerfClientOverSsh(
            ssh_provider=self.device.ssh,
            test_interface=test_interface,
            # Fuchsia's date tool does not support setting system date/time.
            sync_date=False,
        )
