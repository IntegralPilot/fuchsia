# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for FFX transport."""

import logging

import fuchsia_base_test
from honeydew.transports.ffx import types as ffx_types
from honeydew.typing import custom_types
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FFXTransportTests(fuchsia_base_test.FuchsiaBaseTest):
    """FFX transport tests"""

    async def test_check_connection(self) -> None:
        """Test case for FFX.check_connection()."""
        self.dut.ffx.check_connection()

    async def test_get_target_information(self) -> None:
        """Test case for FFX.get_target_information()."""
        self.dut.ffx.get_target_information()

    async def test_get_target_info_from_target_list(self) -> None:
        """Test case for FFX.get_target_info_from_target_list()."""
        self.dut.ffx.get_target_info_from_target_list()

    async def test_get_target_name(self) -> None:
        """Test case for FFX.get_target_name()."""
        asserts.assert_equal(
            self.dut.ffx.get_target_name(), self.dut.device_name
        )

    async def test_get_target_ssh_address(self) -> None:
        """Test case for FFX.get_target_ssh_address()."""
        asserts.assert_is_instance(
            self.dut.ffx.get_target_ssh_address(),
            custom_types.TargetSshAddress,
        )

    async def test_get_target_board(self) -> None:
        """Test case for FFX.get_target_board()."""
        board: str = self.dut.ffx.get_target_board()
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

    async def test_get_target_product(self) -> None:
        """Test case for FFX.get_target_product()."""
        product: str = self.dut.ffx.get_target_product()
        # Note - If "product" is specified in "expected_values" in
        # params.yml then compare with it.
        if self.user_params["expected_values"] and self.user_params[
            "expected_values"
        ].get("product"):
            asserts.assert_equal(
                product, self.user_params["expected_values"]["product"]
            )
        else:
            asserts.assert_is_not_none(product)
            asserts.assert_is_instance(product, str)

    async def test_ffx_run(self) -> None:
        """Test case for FFX.run()."""
        cmd: list[str] = ["target", "ssh", "ls"]
        # `ffx target ssh` does support JSON
        self.dut.ffx.run(cmd, machine=ffx_types.MachineFormat.RAW)

    async def test_ffx_run_subtool(self) -> None:
        """Test case for FFX.run() with a subtool.

        This test requires the test to have `test_data_deps=["//src/developer/ffx/tools/power:ffx_power_test_data"]`
        to ensure the subtool exists.
        """
        cmd: list[str] = ["power", "help"]
        self.dut.ffx.run(cmd)

    async def test_wait_for_rcs_connection(self) -> None:
        """Test case for FFX.wait_for_rcs_connection()."""
        self.dut.ffx.wait_for_rcs_connection()

    async def test_ffx_run_test_component(self) -> None:
        """Test case for FFX.run_test_component()."""
        # Skip test if run on non-eng images.
        asserts.skip_if(
            "core/test_manager" not in self.dut.ffx.run(["component", "list"]),
            "Test manager component is not present",
        )
        output: str = self.dut.ffx.run_test_component(
            "fuchsia-pkg://fuchsia.com/hello-world-rust-tests#meta/hello-world-rust-tests.cm",
        )
        asserts.assert_in("PASSED", output)

    async def test_ffx_run_ssh_cmd(self) -> None:
        """Test case for FFX.run_ssh_cmd()."""
        cmd: str = "ls"
        self.dut.ffx.run_ssh_cmd(cmd)

    async def test_get_ffx_target_status(self) -> None:
        """Test case for FFX.get_ffx_target_status()."""
        output: str = self.dut.ffx.get_ffx_target_status()
        asserts.assert_is_instance(output, str)


if __name__ == "__main__":
    test_runner.main()
