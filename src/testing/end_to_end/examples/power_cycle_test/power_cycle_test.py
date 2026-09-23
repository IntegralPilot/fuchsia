# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Power Cycle test."""

import logging

import fuchsia_base_test
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class PowerCycleTest(fuchsia_base_test.FuchsiaBaseTest):
    """power cycle test.

    Attributes:
        dut: FuchsiaDevice object.

    Required Mobly Test Params:
        num_power_cycles (int): Number of times power_cycle test need to be
            executed.
    """

    async def pre_run(self) -> None:
        """Mobly method used to generate the test cases at run time."""
        test_arg_tuple_list: list[tuple[int]] = []

        for iteration in range(
            1, int(self.user_params["num_power_cycles"]) + 1
        ):
            test_arg_tuple_list.append((iteration,))

        self.generate_tests(
            test_logic=self._test_logic,
            name_func=self._name_func,
            arg_sets=test_arg_tuple_list,
        )

    async def _test_logic(self, iteration: int) -> None:
        """Test case logic that performs power cycle of fuchsia device."""
        _LOGGER.info("Starting the Power Cycle test iteration# %s", iteration)
        await self.dut.power_cycle()
        _LOGGER.info(
            "Successfully ended the Power Cycle test iteration# %s", iteration
        )

    def _name_func(self, iteration: int) -> str:
        """This function generates the names of each test case based on each
        argument set.

        The name function should have the same signature as the actual test
        logic function.

        Returns:
            Test case name
        """
        return f"test_power_cycle_{iteration}"


if __name__ == "__main__":
    test_runner.main()
