# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Fastboot transport."""

import logging

import fuchsia_base_test
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FastbootTransportTests(fuchsia_base_test.FuchsiaBaseTest):
    """Fastboot transport tests"""

    async def setup_class(self) -> None:
        """setup_class is called once before running tests.

        It does the following things:
            * Calls some Fastboot transport method to initialize Fastboot
              transport (as it may involve device reboots)
        """
        await super().setup_class()

        # Calling some fastboot method here, so that Fastboot __init__ gets called which will
        # retrieve the fastboot node-id.
        # Reason for doing this is, if DUT uses TCP based fastboot connection then retrieving the
        # fastboot node-id involves:
        #   * rebooting the device into fastboot mode
        #   * retrieve the fastboot node-id
        #   * reboot back to fuchsia mode
        # So to avoid all these additional steps in actual test case, we are explicitly
        # instantiating fastboot transport in setup_class
        self._fastboot_node_id: str = await self.dut.fastboot.node_id()

    async def teardown_test(self) -> None:
        """teardown_test is called once after running each test.

        It does the following things:
            * Ensures device is in fuchsia mode.
        """
        await super().teardown_test()
        if await self.dut.fastboot.is_in_fastboot_mode():
            _LOGGER.warning(
                "%s is in fastboot mode which is not expected. "
                "Rebooting to fuchsia mode",
                self.dut.device_name,
            )
            await self.dut.fastboot.boot_to_fuchsia_mode()

    async def test_fastboot_node_id(self) -> None:
        """Test case for Fastboot.node_id."""
        # Note - If "node_id" is specified in "expected_values" in
        # params.yml then compare with it.
        if self.user_params["expected_values"] and self.user_params[
            "expected_values"
        ].get("node_id"):
            asserts.assert_equal(
                self._fastboot_node_id,
                self.user_params["expected_values"]["node_id"],
            )
        else:
            asserts.assert_is_not_none(self._fastboot_node_id)
            asserts.assert_is_instance(self._fastboot_node_id, str)

    async def test_fastboot_methods(self) -> None:
        """Test case that puts the device in fastboot mode, runs a command in
        fastboot mode and reboots the device back to fuchsia mode."""
        await self.dut.fastboot.boot_to_fastboot_mode()

        await self.dut.fastboot.wait_for_fastboot_mode()

        asserts.assert_true(
            await self.dut.fastboot.is_in_fastboot_mode(),
            msg=f"{self.dut.device_name} is not in fastboot mode which "
            f"is not expected",
        )

        cmd: list[str] = ["getvar", "hw-revision"]
        await self.dut.fastboot.run(cmd)

        await self.dut.fastboot.boot_to_fuchsia_mode()

        await self.dut.fastboot.wait_for_fuchsia_mode()

        asserts.assert_false(
            await self.dut.fastboot.is_in_fastboot_mode(),
            msg=f"{self.dut.device_name} is in fastboot mode when not "
            f"expected",
        )


if __name__ == "__main__":
    test_runner.main()
