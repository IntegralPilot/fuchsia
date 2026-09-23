# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for UserInput affordance."""

import os
from typing import Optional

import fuchsia_base_test
from honeydew import errors
from honeydew.affordances.ui.screenshot import types
from honeydew.affordances.ui.user_input import types as ui_custom_types
from honeydew.utils import common
from mobly import asserts, test_runner

INPUT_APP = (
    "fuchsia-pkg://fuchsia.com/flatland-examples#meta/"
    "simplest-app-flatland-session.cm"
)


class UserInputTestCases(fuchsia_base_test.FuchsiaTestCases):
    """Test logic for UserInput affordance."""

    async def setup_test(
        self,
    ) -> None:
        await super().setup_test()
        self.screenshot_attempt_count = 0

    def _take_and_save_screenshot(
        self, name_prefix: str, attempt_num: Optional[int] = None
    ) -> types.ScreenshotImage:
        """Takes a screenshot and saves it with a unique name.
        If an image is provided, it just saves it.
        """
        image = self.dut.screenshot.take()

        if attempt_num is not None:
            file_name = f"screenshot-{name_prefix}-{attempt_num}.png"
        else:
            file_name = f"screenshot-{name_prefix}.png"

        image.save(os.path.join(self.test_case_path, file_name))
        return image

    async def _wait_for_pixel_change(
        self, before: types.ScreenshotImage, tag: str
    ) -> None:
        """Waits for the top-left pixel to change from the 'before' screenshot.

        Args:
            before: The screenshot taken before the action.
            tag: A descriptive tag for the screenshot (e.g., 'tap', 'swipe').
        """

        def pixel_changed_condition() -> bool:
            current_screenshot = self._take_and_save_screenshot(
                f"after_{tag}", self.screenshot_attempt_count
            )
            self.screenshot_attempt_count += 1
            return before.data[0:4] != current_screenshot.data[0:4]

        try:
            await common.wait_for_state(
                state_fn=pixel_changed_condition,
                expected_state=True,
                wait_time=2,
            )
        except errors.HoneydewTimeoutError:
            asserts.fail(f"color did not change after {tag} within timeout")

    async def _click_to_focus(self) -> None:
        """Clicks on the screen to focus the app, and waits for color change."""
        self.mouse_device = self.dut.user_input.create_mouse_device()
        before_click = self._take_and_save_screenshot("before_click_for_focus")
        await self.mouse_device.click()

        await self._wait_for_pixel_change(before_click, "click_for_focus")

    async def test_user_input_tap(self) -> None:
        self.dut.session.add_component(INPUT_APP)

        # The app will change the color when a tap is received.
        # Ensure the top left pixel changes after tap
        before = self._take_and_save_screenshot("before_tap")

        touch_device = self.dut.user_input.create_touch_device()
        await touch_device.tap(
            location=ui_custom_types.Coordinate(x=1, y=2), tap_event_count=1
        )

        await self._wait_for_pixel_change(before, "tap")

    async def test_user_input_swipe(self) -> None:
        self.dut.session.add_component(INPUT_APP)

        # The app will change the color when a tap is received.
        # Ensure the top left pixel changes after tap
        before = self._take_and_save_screenshot("before_swipe")

        touch_device = self.dut.user_input.create_touch_device()

        await touch_device.swipe(
            start_location=ui_custom_types.Coordinate(x=1, y=2),
            end_location=ui_custom_types.Coordinate(x=3, y=4),
            move_event_count=2,
            duration_ms=2000,
        )

        await self._wait_for_pixel_change(before, "swipe")

    async def test_user_input_press_key(self) -> None:
        self.dut.session.add_component(INPUT_APP)

        keyboard_device = self.dut.user_input.create_keyboard_device()
        before_keypress = self._take_and_save_screenshot("before_keypress")

        await keyboard_device.key_press(key_code=0x00070004)  # Key A

        await self._wait_for_pixel_change(before_keypress, "keypress")

    async def test_user_input_mouse_click(self) -> None:
        self.dut.session.add_component(INPUT_APP)
        mouse_device = self.dut.user_input.create_mouse_device()

        # The app will change the color when a click is received.
        # Ensure the top left pixel changes after click
        before = self._take_and_save_screenshot("before_mouse_click")

        await mouse_device.click()

        await self._wait_for_pixel_change(before, "mouse_click")

    async def test_user_input_mouse_scroll(self) -> None:
        self.dut.session.add_component(INPUT_APP)
        await self._click_to_focus()

        # Now get the color before scroll
        before_scroll = self._take_and_save_screenshot("before_scroll")

        # Simulate a scroll event. If the underlying FIDL connection or
        # registry fails, this will raise a UserInputError and fail the test.
        await self.mouse_device.scroll(scroll_v_detent=10)

        await self._wait_for_pixel_change(before_scroll, "scroll")


class UserInputAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """UserInput affordance tests"""

    TEST_CASES = [UserInputTestCases]

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
