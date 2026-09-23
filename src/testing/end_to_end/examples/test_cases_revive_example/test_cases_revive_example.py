# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Example test demonstrating FuchsiaTestCases and TestCaseRevive."""

import logging

import fuchsia_base_test
import test_case_revive
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class MyTestCases(fuchsia_base_test.FuchsiaTestCases):
    """Example test cases."""

    async def setup_test(self) -> None:
        await super().setup_test()
        _LOGGER.info("MyTestCases.setup_test() called")

    async def teardown_test(self) -> None:
        _LOGGER.info("MyTestCases.teardown_test() called")
        await super().teardown_test()

    async def test_case_one(self) -> None:
        _LOGGER.info("Executing test_case_one")

    @test_case_revive.tag_test(
        fuchsia_device_operation=test_case_revive.FuchsiaDeviceOperation.SOFT_REBOOT,
        test_method_execution_frequency=test_case_revive.TestMethodExecutionFrequency.PRE_AND_POST,
    )
    async def test_revive_me(self) -> None:
        _LOGGER.info("Executing test_revive_me")


class ExampleTest(test_case_revive.TestCaseRevive):
    """Example test using FuchsiaTestCases with TestCaseRevive."""

    TEST_CASES = [MyTestCases]


if __name__ == "__main__":
    test_runner.main()
