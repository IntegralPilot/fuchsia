# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Screenshot affordance."""

import asyncio
import logging
import os

import fuchsia_base_test
from honeydew.typing import custom_types
from mobly import asserts, test_runner

_LOGGER = logging.getLogger(__name__)

EXAMPLE_URL = (
    "fuchsia-pkg://fuchsia.com/flatland-examples#meta/"
    "simplest-app-flatland.cm"
)


class ScreenshotTestCases(fuchsia_base_test.FuchsiaTestCases):
    """Test logic for Screenshot affordance."""

    async def test_take_screenshot(self) -> None:
        # We launch the test app that draws something on the screen.
        # EXAMPLE_URL(flatland-examples) will render colorful background instead
        # of blank screen. It is better for screenshot to verify
        # "It really take a screenshot" instead of "It just give an empty pic".

        _LOGGER.info("Launching %s", EXAMPLE_URL)
        await self.dut.log_message_to_device(
            f"Launching test app {EXAMPLE_URL}...", custom_types.LEVEL.INFO
        )
        self.dut.session.add_component(EXAMPLE_URL)

        # Give the component a chance to load
        # TODO(b/320583170): Can be removed once we have APIs to check for component
        # actually running.
        _LOGGER.info("Waiting for test app to load...")
        await asyncio.sleep(10)

        _LOGGER.info("Taking screenshot...")
        await self.dut.log_message_to_device(
            "Taking screenshot...", custom_types.LEVEL.INFO
        )
        image = self.dut.screenshot.take()

        # Save screenshot for debugging
        file_name = f"screenshot_{image.size.width}x{image.size.height}.png"

        _LOGGER.info("Saving screenshot to %s.", file_name)
        image.save(os.path.join(self.test_case_path, file_name))

        asserts.assert_greater(image.size.width, 0)
        asserts.assert_greater(image.size.height, 0)
        asserts.assert_equal(
            image.size.width * image.size.height * 4, len(image.data)
        )

        # Example app render a colorful image with color HSV(?, 75 or 30, 75).
        # Ensure the top left pixel is not black or transparent.
        asserts.assert_not_equal(image.data[0:4], [0x0, 0x0, 0x0, 0xFF])


class ScreenshotAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """Screenshot affordance tests"""

    TEST_CASES = [ScreenshotTestCases]

    async def setup_class(self) -> None:
        """setup_class is called once before running tests.

        It does the following things:
            * Assigns `dut` variable with FuchsiaDevice object
        """
        await super().setup_class()

    async def setup_test(self) -> None:
        await super().setup_test()
        self.dut.session.ensure_started()

    async def teardown_test(self) -> None:
        self.dut.session.cleanup()
        await super().teardown_test()


if __name__ == "__main__":
    test_runner.main()
