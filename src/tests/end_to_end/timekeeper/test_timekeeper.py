# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Timekeeper tests for Lacewing.

The first test is to verify that a device is reachable and that Timekeeper
is exporting at least some metrics. This is an indication that Timekeeper is
operational.

"""

import logging

import fuchsia_base_test
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)

_TIMEOUT = 20.0  # seconds(?)


class TimekeeperTest(fuchsia_base_test.FuchsiaBaseTest):
    def test_check_timekeeper_present(self) -> None:
        for fuchsia_device in self.fuchsia_devices:
            _LOGGER.info("%s says hello!", fuchsia_device.device_name)
            ffx = fuchsia_device.ffx

            # `ffx component show core/timekeeper` may give output even if
            # Timekeeper is not running.
            output = ffx.run(
                ["inspect", "show", "core/timekeeper"],
                capture_output=True,
                timeout=_TIMEOUT,
            )
            asserts.assert_not_equal(
                output,
                "",
                msg="Expected to see nonempty Timekeeper inspect metrics",
            )
            asserts.assert_not_equal(
                output,
                "\n",
                msg="Expected to see nonempty Timekeeper inspect metrics",
            )


if __name__ == "__main__":
    test_runner.main()
