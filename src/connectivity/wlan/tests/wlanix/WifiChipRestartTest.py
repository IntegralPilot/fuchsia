# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Test that the WifiChip can be restarted.
"""

import asyncio
import logging
from typing import Any

import fidl_fuchsia_wlan_wlanix as fidl_wlanix
from fuchsia_controller_py import Channel
from mobly import test_runner
from wlanix_testing import base_test


class WifiEventCallbackServer(fidl_wlanix.WifiEventCallbackServer):
    def __init__(self, channel: Channel) -> None:
        super().__init__(channel)
        self.subsystem_restart_received = asyncio.Event()

    def on_subsystem_restart(self, payload: Any) -> None:
        logging.info(f"Received OnSubsystemRestart: payload={payload}")
        self.subsystem_restart_received.set()

    def on_start(self) -> None:
        pass

    def on_stop(self) -> None:
        pass


class WifiChipRestartTest(base_test.WifiChipBaseTestClass):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, allow_ifaces_between_tests=True, **kwargs)

    async def test_trigger_subsystem_restart(self) -> None:
        """Test calling TriggerSubsystemRestart API."""

        # Connect to Wifi to register callback
        (
            proxy,
            server,
        ) = self.dut.fuchsia_controller.channel_create()
        self.wlanix_proxy.get_wifi(wifi=server.take())
        wifi_proxy = fidl_wlanix.WifiClient(proxy)

        # Register callback
        (
            callback_client_end,
            callback_server_end,
        ) = self.dut.fuchsia_controller.channel_create()
        wifi_proxy.register_event_callback(callback=callback_client_end.take())

        my_callback_server = WifiEventCallbackServer(callback_server_end)
        serve_task = asyncio.create_task(my_callback_server.serve())

        # Ensure the event callback registration is fully processed
        # by making a two-way call on the same channel before triggering restart.
        (await wifi_proxy.get_state()).unwrap()

        try:
            logging.info("Triggering subsystem restart...")
            # Trigger restart
            (await self.wifi_chip_proxy.trigger_subsystem_restart()).unwrap()
            logging.info("Restart triggered.")

            # Wait for subsystem restart
            try:
                await asyncio.wait_for(
                    my_callback_server.subsystem_restart_received.wait(),
                    timeout=30,
                )
                logging.info("OnSubsystemRestart event received.")
            except asyncio.TimeoutError:
                from mobly import signals

                raise signals.TestFailure(
                    "Timed out waiting for OnSubsystemRestart event"
                )
        finally:
            # Clean up serve task
            serve_task.cancel()
            try:
                await serve_task
            except asyncio.CancelledError:
                pass


if __name__ == "__main__":
    test_runner.main()
