# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Example test demonstrating FuchsiaTestCases."""

import logging

import fuchsia_base_test
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class ExampleTestCases(fuchsia_base_test.FuchsiaTestCases):
    """Example test cases."""

    async def setup_test(
        self,
    ) -> None:
        await super().setup_test()
        _LOGGER.info("ExampleTestCases.setup_test() called")

    async def teardown_test(self) -> None:
        _LOGGER.info("ExampleTestCases.teardown_test() called")
        await super().teardown_test()

    async def test_example_case(self) -> None:
        for fuchsia_device in self.fuchsia_devices:
            _LOGGER.info(
                "%s inside test_example_case!", fuchsia_device.device_name
            )


class ExampleTest(fuchsia_base_test.FuchsiaBaseTest):
    """Example test using TEST_CASES."""

    TEST_CASES = [ExampleTestCases]


if __name__ == "__main__":
    test_runner.main()
