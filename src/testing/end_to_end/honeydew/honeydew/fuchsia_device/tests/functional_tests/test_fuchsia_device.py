# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for fuchsia_device.py device class."""

import logging
import os
import tempfile
from typing import Any

import fuchsia_base_test
import fuchsia_inspect
from honeydew.typing import custom_types
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

# Note - Following destructive APIs in FuchsiaDevice class should have its own
# test class to make sure failure of those destructive APIs does not impact
# the rest of the non-destructive APIs tests:
# * `reboot()` - Test class is @ <>/end_to_end/examples/test_soft_reboot/
# * `power_cycle()`

# Note - Below APIs will be tested automatically in `.reboot()` test case
#   * `on_device_boot()`
#   * `wait_for_offline()`

# Note - Do not add separate functional test for `close()` as it will clean up
# the FuchsiaDevice Honeydew object and thus any subsequent calls will fail.
# `close()` is called anyway when Mobly calls `destroy()` defined in the
# FuchsiaDevice mobly controller

# Note - `register_for_on_device_boot()` has been fully tested using unit test

# Note - To properly test `resolve_device_ip()`, call it after fuchsia device IP address is changed.
# However, in infra it is guaranteed to have the device ip address same through out the test
# execution. So functional test for this currently checks that calling this method (even when
# ip address is not changed) will not break anything from host-target communication perspective.
# Proper test for this method will be called by OTA test (that is run on emulator which results in
# changing the IP address of the device).


# pylint: disable=pointless-statement
class FuchsiaDeviceTests(fuchsia_base_test.FuchsiaBaseTest):
    """FuchsiaDevice tests"""

    async def test_board(self) -> None:
        """Test case for board"""
        board: str = self.dut.board
        # Note - If "board" is specified in "expected_values" in
        # params.yml then compare with it.
        if self.user_params["expected_values"] and self.user_params[
            "expected_values"
        ].get("board"):
            asserts.assert_equal(
                board, self.user_params["expected_values"]["board"]
            )
        else:
            asserts.assert_is_not_none(board)
            asserts.assert_is_instance(board, str)

    async def test_manufacturer(self) -> None:
        """Test case for manufacturer"""
        asserts.assert_equal(
            await self.dut.manufacturer(),
            self.user_params["expected_values"]["manufacturer"],
        )

    async def test_model(self) -> None:
        """Test case for model"""
        asserts.assert_equal(
            await self.dut.model(),
            self.user_params["expected_values"]["model"],
        )

    async def test_product(self) -> None:
        """Test case for product"""
        product: str = self.dut.product
        asserts.assert_is_not_none(product)
        asserts.assert_is_instance(product, str)

    async def test_product_name(self) -> None:
        """Test case for product_name"""
        asserts.assert_equal(
            await self.dut.product_name(),
            self.user_params["expected_values"]["product_name"],
        )

    async def test_serial_number(self) -> None:
        """Test case for serial_number"""
        # Note - Some devices such as FEmu, X64 does not have a serial_number.
        asserts.assert_true(
            isinstance(await self.dut.serial_number(), (str, type(None))),
            msg="serial_number operation failed",
        )

    async def test_firmware_version(self) -> None:
        """Test case for firmware_version"""
        # Note - If "firmware_version" is specified in "expected_values" in
        # params.yml then compare with it.
        if "firmware_version" in self.user_params["expected_values"]:
            asserts.assert_equal(
                await self.dut.firmware_version(),
                self.user_params["expected_values"]["firmware_version"],
            )
        else:
            asserts.assert_is_instance(await self.dut.firmware_version(), str)

    async def test_last_reboot_reason(self) -> None:
        """Test case for last_reboot_reason"""
        # It's unclear how much this functional test should be asserting. Is it
        # about whether the reboot reason returned here is the one on the
        # device? Is it about whether the device rebooted with the right reason?
        # Given that we have //src/tests/end_to_end/reboot_reason to test the
        # whole reboot flow and the reason, it's fine here to just assert that
        # a string reboot reason should always be available.
        asserts.assert_is_instance(await self.dut.last_reboot_reason(), str)

    async def test_is_starnix_device(self) -> None:
        """Test case for is_starnix_device"""
        asserts.assert_is_instance(self.dut.is_starnix_device(), bool)

    async def test_health_check(self) -> None:
        """Test case for health_check()"""
        self.dut.health_check()

    async def test_get_inspect_data_without_selectors_and_monikers(
        self,
    ) -> None:
        inspect_data_collection: fuchsia_inspect.InspectDataCollection = (
            self.dut.get_inspect_data()
        )

        asserts.assert_is_instance(
            inspect_data_collection, fuchsia_inspect.InspectDataCollection
        )
        for inspect_data in inspect_data_collection.data:
            asserts.assert_is_instance(
                inspect_data,
                fuchsia_inspect.InspectData,
            )

    async def test_get_inspect_data_with_one_selector_validate_schema(
        self,
    ) -> None:
        class _AnyUnsignedInteger:
            def __eq__(self, other: object) -> bool:
                return isinstance(other, int) and other >= 0

        inspect_data_collection: fuchsia_inspect.InspectDataCollection = (
            self.dut.get_inspect_data(
                selectors=["bootstrap/archivist:root/fuchsia.inspect.Health"],
            )
        )

        expected_payload: dict[str, Any] = {
            "root": {
                "fuchsia.inspect.Health": {
                    "start_timestamp_nanos": _AnyUnsignedInteger(),
                    "status": "OK",
                }
            }
        }
        fuchsia_inspect_health_data: fuchsia_inspect.InspectData = (
            inspect_data_collection.data[0]
        )
        asserts.assert_equal(
            fuchsia_inspect_health_data.payload,
            expected_payload,
            f"Expected payload: ####{expected_payload}#### but "
            f"received payload: ####{fuchsia_inspect_health_data.payload}####",
        )

    async def test_get_inspect_data_with_multiple_selectors(self) -> None:
        selectors: list[str] = [
            "bootstrap/fshost",
            "bootstrap/archivist",
        ]

        inspect_data_collection: fuchsia_inspect.InspectDataCollection = (
            self.dut.get_inspect_data(
                selectors=selectors,
            )
        )

        monikers: list[str] = [
            inspect_data.moniker
            for inspect_data in inspect_data_collection.data
        ]
        asserts.assert_equal(sorted(monikers), sorted(selectors))

    async def test_log_message_to_device(self) -> None:
        """Test case for log_message_to_device()"""
        await self.dut.log_message_to_device(
            message="This is a test ERROR message",
            level=custom_types.LEVEL.ERROR,
        )

        await self.dut.log_message_to_device(
            message="This is a test WARNING message",
            level=custom_types.LEVEL.WARNING,
        )

        await self.dut.log_message_to_device(
            message="This is a test INFO message", level=custom_types.LEVEL.INFO
        )

    async def test_snapshot(self) -> None:
        """Test case for snapshot()"""
        with tempfile.TemporaryDirectory() as tmpdir:
            await self.dut.snapshot(
                directory=tmpdir, snapshot_file="snapshot.zip"
            )
            exists: bool = os.path.exists(f"{tmpdir}/snapshot.zip")
        asserts.assert_true(exists, msg="snapshot failed")

    async def test_wait_for_online(self) -> None:
        """Test case for wait_for_online()"""
        await self.dut.wait_for_online()

    async def test_resolve_device_ip(self) -> None:
        """Test case for resolve_device_ip()"""
        await self.dut.resolve_device_ip()


if __name__ == "__main__":
    test_runner.main()
