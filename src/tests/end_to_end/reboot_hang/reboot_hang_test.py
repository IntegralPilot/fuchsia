# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import json
import logging

import fuchsia_base_test
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class RebootHangTest(fuchsia_base_test.FuchsiaBaseTest):
    async def setup_test(self) -> None:
        await super().setup_test()
        self.device = self.fuchsia_devices[0]
        # Initialize fastboot transport while the device is still online.
        await self.device.fastboot.make_ready()

    async def test_reboot_to_bootloader_on_hang(self) -> None:
        # 0. Register the test driver (since it is ephemeral).
        _LOGGER.info("Registering hang-on-stop driver...")
        self.device.ffx.run(
            [
                "driver",
                "register",
                "fuchsia-pkg://fuchsia.com/hang-on-stop#meta/hang-on-stop.cm",
            ]
        )

        # 1. Dynamically create the virtual parent device to bind the hang driver.
        _LOGGER.info("Adding test node to trigger hang driver...")
        self.device.ffx.run(
            [
                "driver",
                "test-node",
                "add",
                "hang_parent",
                "fuchsia.test.TEST_CHILD=hang_parent",
            ]
        )

        # Verify driver bound
        _LOGGER.info("Verifying hang-on-stop driver is bound...")

        bound = False
        for i in range(10):
            try:
                stdout = self.device.ffx.run(
                    ["driver", "node", "show", "hang_parent"]
                )
                nodes = json.loads(stdout)
                if (
                    nodes
                    and nodes[0].get("owner")
                    == "fuchsia-pkg://fuchsia.com/hang-on-stop#meta/hang-on-stop.cm"
                ):
                    bound = True
            except Exception as e:
                _LOGGER.warning(f"Error during driver binding check: {e}")

            if bound:
                _LOGGER.info(
                    "hang-on-stop driver successfully bound to hang_parent!"
                )
                break
            await asyncio.sleep(1)

        asserts.assert_true(
            bound, "Failed to bind hang-on-stop driver to hang_parent"
        )

        # 2. Trigger reboot to bootloader.
        # The reboot command may succeed before the driver host hangs. Since we
        # verified the driver is bound above, we know the hang is guaranteed.
        # We ignore the result and wait for the device to go offline.
        _LOGGER.info("Triggering reboot to bootloader...")
        try:
            self.device.ffx.run(
                ["target", "reboot", "--bootloader"], timeout=30
            )
        except Exception as e:
            _LOGGER.info(
                f"Reboot command finished with exception (likely connection lost): {e}"
            )

        # 3. Wait for device to go offline.
        _LOGGER.info("Waiting for device to go offline...")
        # wait_for_offline is synchronous in Honeydew, so we run it in a thread.
        # (wait_for_online is async and is awaited directly below).
        await asyncio.to_thread(self.device.wait_for_offline)

        # 4. Wait for device to enter Fastboot.
        # We use a generous timeout to allow the device to reboot and enter fastboot.
        _LOGGER.info("Waiting for device to enter Fastboot mode...")
        fastboot_transport = self.device.fastboot
        try:
            node_id = await fastboot_transport.node_id()
            _LOGGER.info(f"Expected Fastboot Node ID: {node_id}")
        except Exception as e:
            _LOGGER.warning(f"Could not retrieve fastboot node_id: {e}")

        try:
            await asyncio.wait_for(
                fastboot_transport.wait_for_fastboot_mode(),
                timeout=180.0,
            )
        except asyncio.TimeoutError:
            try:
                out = self.device.ffx.run(["target", "list"])
                _LOGGER.info(f"ffx target list output: {out}")
            except Exception as ex:
                _LOGGER.warning(f"Failed to run ffx target list: {ex}")
            asserts.fail("Timed out waiting for device to enter Fastboot mode")

        # 5. Assert we are in fastboot.
        asserts.assert_true(
            await fastboot_transport.is_in_fastboot_mode(),
            "Device failed to boot into fastboot",
        )

        # 6. Recovery: Reboot back to Fuchsia.
        _LOGGER.info("Rebooting back to Fuchsia...")
        await fastboot_transport.boot_to_fuchsia_mode()
        await self.device.wait_for_online()
        _LOGGER.info("Device is back online.")

    async def teardown_test(self) -> None:
        # Ensure we always attempt to recover the device if it got stuck in fastboot
        try:
            fastboot_transport = self.device.fastboot
            if await fastboot_transport.is_in_fastboot_mode():
                _LOGGER.warning(
                    "Device stuck in fastboot during teardown, attempting recovery..."
                )
                await fastboot_transport.boot_to_fuchsia_mode()
                await self.device.wait_for_online()
        except Exception as e:
            _LOGGER.error(f"Failed to recover device in teardown: {e}")
        await super().teardown_test()


if __name__ == "__main__":
    test_runner.main()
