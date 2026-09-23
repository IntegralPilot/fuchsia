# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import logging

import fuchsia_wlan_base_test
import honeydew.affordances.connectivity.wlan.core as wlan_core
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from mobly import test_runner
from mobly.asserts import assert_true

logger = logging.getLogger(__name__)


class WlanDriverRestartTest(fuchsia_wlan_base_test.FuchsiaWlanBaseTest):
    phy: wlan_core.Phy | None

    async def setup_test(self) -> None:
        await super().setup_test()
        await self.dut.wlan_core.destroy_all_ifaces()

    async def test_driver_host_restart(self) -> None:
        # TODO(b/494309251): If a driver debug side channel is created, use that to query out the
        # KOID and kill that instead.
        logger.info("Restarting WLAN driver...")
        processes = self.dut.ffx.run_ssh_cmd("ps")
        wlan_driver_process = None
        keywords = ["iwlwifi", "realtek", "brcmfmac"]
        for line in processes.splitlines():
            if any(k in line for k in keywords):
                # The process name is exactly the last token, e.g. "iwlwifi.cm"
                wlan_driver_process = line.split()[-1]
                break

        if wlan_driver_process:
            logger.info(
                f"Detected running DFv2 WLAN process: {wlan_driver_process}. Killing via SSH..."
            )
            self.dut.ffx.run_ssh_cmd(f"killall {wlan_driver_process}")
        elif "driver-host-#wlan" in processes:
            logger.info("Detected legacy DFv1 driver host. Killing via SSH...")
            self.dut.ffx.run_ssh_cmd("killall driver-host-#wlan")
        else:
            raise RuntimeError(
                "Could not find any running WLAN driver process to restart!"
            )

        logger.info("Polling for PHY to be removed")
        phy_removal_timeout = 10
        poll_interval = 0.25

        for _ in range(int(phy_removal_timeout / poll_interval)):
            try:
                await self.dut.wlan_core.ensure_single_phy()
            except HoneydewWlanError:
                logger.info("Successfully observed PHY removal")
                break

            await asyncio.sleep(poll_interval)
        else:
            assert_true(False, "Timed out waiting for PHYs to be removed")

        logger.info("Polling for PHY to return")
        phy_return_timeout = 30
        poll_interval = 1.0
        phy = None

        for _ in range(int(phy_return_timeout / poll_interval)):
            try:
                phy = await self.dut.wlan_core.ensure_single_phy()
                logger.info(f"Successfully found restored PHY: {phy}")
                break
            except HoneydewWlanError:
                pass

            await asyncio.sleep(poll_interval)
        else:
            assert_true(False, "Timed out waiting for PHYs to be restored")

        assert phy is not None

        logger.info("Creating client interface...")
        iface = await phy.create_client_iface()

        logger.info("Issuing scan request...")
        await iface.passive_scan()

        # Completing the passive_scan with no exception raised means
        # the scan succeeded!


if __name__ == "__main__":
    test_runner.main()
