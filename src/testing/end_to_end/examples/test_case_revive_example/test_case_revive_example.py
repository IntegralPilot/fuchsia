# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Example usage of test_case_revive.TestCaseRevive."""

import logging

import test_case_revive
from mobly import test_runner
from test_case_revive import TestMethodExecutionFrequency, opt_out, tag_test

_LOGGER: logging.Logger = logging.getLogger(__name__)


class ExampleTestCaseRevive(test_case_revive.TestCaseRevive):
    """Example usage of test_case_revive.TestCaseRevive."""

    def _to_run_before_test(self, foo: int) -> None:
        _LOGGER.info(
            "This line should get printed at the beginning of the revived test. DUT name is: %s, foo = %s.",
            self.dut.device_name,
            foo,
        )

    def _to_run_after_test(self, bar: int) -> None:
        _LOGGER.info(
            "This line should get printed at the end of the revived test. DUT name is: %s, bar = %s",
            self.dut.device_name,
            bar,
        )

    @opt_out()
    def test_that_does_not_revive(self) -> None:
        """This test case will not be run if built with `params.test_case_revive = true`"""
        _LOGGER.info(
            "This should run in normal Mobly but not run in 'revived' mode"
        )

    async def test_firmware_version(self) -> None:
        """This test can be run as a normal or revived test.

        If built without `params.test_case_revive = true` it will run normally once.

        If built with `params.test_case_revive = true` it will be run twice in this sequence:
            1. Run this test case method
            2. Perform `fuchsia_device_operation` operation
            3. Re-run this test case method

        To prevent running a test case in revive mode even if built with the revive flag,
        annotate it with `@opt_out()`.
        """
        for fuchsia_device in self.fuchsia_devices:
            _LOGGER.info(
                "%s is running on %s firmware",
                fuchsia_device.device_name,
                await fuchsia_device.firmware_version(),
            )

    @tag_test(
        test_method_execution_frequency=TestMethodExecutionFrequency.POST_ONLY,
        pre_test_execution_fn=_to_run_before_test,
        pre_test_execution_fn_kwargs={"foo": 1},
        post_test_execution_fn=_to_run_after_test,
        post_test_execution_fn_kwargs={"bar": 1},
    )
    def _test_run_only_with_revive_option_with_pre_and_post_test_execution_fn_and_kwargs(
        self,
    ) -> None:
        """This will be run only as a revived test case and runs the following
        sequence:

            1. Run _to_run_before_test method
            2. Perform `fuchsia_device_operation` operation
            3. Run this test case method
            4. Run _to_run_after_test method

        Since the function name starts with "_test" and not "test" Mobly will not
        recognize it as a test case. But if built with `params.test_case_revive = true`
        the revive mechanism will use it as a test case.
        """
        for fuchsia_device in self.fuchsia_devices:
            _LOGGER.info(
                "%s is a %s board",
                fuchsia_device.device_name,
                fuchsia_device.board,
            )


if __name__ == "__main__":
    test_runner.main()
