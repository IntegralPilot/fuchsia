# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.fuchsia_controller.user_input.py."""

import types
import unittest
from unittest import mock

import fidl_fuchsia_math as f_math
import fidl_fuchsia_ui_test_input as f_test_input
import fuchsia_controller_py
from honeydew import errors
from honeydew.affordances.ui.user_input import errors as user_input_errors
from honeydew.affordances.ui.user_input import types as ui_custom_types
from honeydew.affordances.ui.user_input import user_input_using_fc
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.transports.fuchsia_controller import (
    fuchsia_controller as fc_transport,
)
from honeydew.typing import custom_types


# pylint: disable=protected-access
class UserInputFCTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.affordances.fuchsia_controller.ui.user_input.py."""

    async def asyncSetUp(self) -> None:
        super().setUp()
        self.fc_transport_obj = mock.MagicMock(
            spec=fc_transport.FuchsiaController
        )
        self.fc_transport_obj.ctx = fuchsia_controller_py.Context()

        def channel_create(
            self: fc_transport.FuchsiaController,
        ) -> tuple[
            fuchsia_controller_py.Channel, fuchsia_controller_py.Channel
        ]:
            return self.ctx.channel_create()

        self.fc_transport_obj.channel_create = types.MethodType(
            channel_create, self.fc_transport_obj
        )
        self.ffx_transport_obj = mock.MagicMock(spec=ffx_transport.FFX)

    async def test_verify_supported_no_virtual_device_support_raise_error(
        self,
    ) -> None:
        """Test for user_input_using_fc.UserInputUsingFc() method raise error without virtual device
        support."""

        self.ffx_transport_obj.run.return_value = '{"instances": []}'
        with self.assertRaises(errors.NotSupportedError):
            user_input_using_fc.UserInputUsingFc(
                device_name="fuchsia-emulator",
                fuchsia_controller=self.fc_transport_obj,
                ffx_transport=self.ffx_transport_obj,
            )

        self.ffx_transport_obj.run.assert_called_once_with(
            ["component", "list"]
        )

    async def test_verify_supported_no_raise(self) -> None:
        """Test for user_input_using_fc.UserInputUsingFc() method not raise error with virtual
        device support."""
        self.ffx_transport_obj.run.return_value = f'{{"instances": [{{"moniker": "{user_input_using_fc._INPUT_HELPER_COMPONENT}"}}]}}'
        user_input_using_fc.UserInputUsingFc(
            device_name="fuchsia-emulator",
            fuchsia_controller=self.fc_transport_obj,
            ffx_transport=self.ffx_transport_obj,
        )

    def user_input(self) -> user_input_using_fc.UserInputUsingFc:
        self.ffx_transport_obj.run.return_value = f'{{"instances": [{{"moniker": "{user_input_using_fc._INPUT_HELPER_COMPONENT}"}}]}}'
        return user_input_using_fc.UserInputUsingFc(
            device_name="fuchsia-emulator",
            fuchsia_controller=self.fc_transport_obj,
            ffx_transport=self.ffx_transport_obj,
        )

    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_touch_screen",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_create_touch_device(self, register_touch_screen) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.create_touch_device() method."""

        touch_device = self.user_input().create_touch_device()
        await touch_device.make_ready()

        self.fc_transport_obj.connect_device_proxy.assert_called_once_with(
            custom_types.FidlEndpoint(
                "/core/ui", "fuchsia.ui.test.input.Registry"
            )
        )

        register_touch_screen.assert_called_once()

        self.assertIsNotNone(touch_device._touch_screen_proxy)

    @mock.patch.object(
        f_test_input.TouchScreenClient,
        "simulate_touch_event",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_touch_screen",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_tap_only_required(self, unused_register_touch_screen, simulate_touch_event) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.tap() method with only required params."""

        touch_device = self.user_input().create_touch_device()
        await touch_device.tap(location=ui_custom_types.Coordinate(x=1, y=2))
        simulate_touch_event.assert_has_calls(
            [
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[
                            f_test_input.ContactInputReport(
                                contact_id=1,
                                position_x=1,
                                position_y=2,
                            ),
                        ],
                    ),
                ),
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[],
                    ),
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.TouchScreenClient,
        "simulate_touch_event",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_touch_screen",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_tap_all_params(self, unused_register_touch_screen, simulate_touch_event) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.tap() method with all params."""

        touch_device = self.user_input().create_touch_device(
            touch_screen_size=ui_custom_types.Size(width=3, height=4),
        )
        await touch_device.tap(
            location=ui_custom_types.Coordinate(x=1, y=2),
            tap_event_count=3,
            duration_ms=6,
            duration_of_one_tap_ms=2,
        )
        simulate_touch_event.assert_has_calls(
            [
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[
                            f_test_input.ContactInputReport(
                                contact_id=1,
                                position_x=1,
                                position_y=2,
                            ),
                        ],
                    ),
                ),
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[],
                    ),
                ),
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[
                            f_test_input.ContactInputReport(
                                contact_id=1,
                                position_x=1,
                                position_y=2,
                            ),
                        ],
                    ),
                ),
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[],
                    ),
                ),
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[
                            f_test_input.ContactInputReport(
                                contact_id=1,
                                position_x=1,
                                position_y=2,
                            ),
                        ],
                    ),
                ),
                mock.call(
                    report=f_test_input.TouchInputReport(
                        contacts=[],
                    ),
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.TouchScreenClient,
        "simulate_swipe",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_touch_screen",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_swipe(self, unused_register_touch_screen, simulate_swipe) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.swipe() method."""

        touch_device = self.user_input().create_touch_device(
            touch_screen_size=ui_custom_types.Size(width=4, height=5),
        )
        await touch_device.swipe(
            start_location=ui_custom_types.Coordinate(x=1, y=2),
            end_location=ui_custom_types.Coordinate(x=3, y=4),
            move_event_count=2,
        )
        simulate_swipe.assert_has_calls(
            [
                mock.call(
                    start_location=f_math.Vec(x=1, y=2),
                    end_location=f_math.Vec(x=3, y=4),
                    move_event_count=2,
                    duration=0,
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.TouchScreenClient,
        "simulate_swipe",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_touch_screen",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_swipe_with_duration(self, unused_register_touch_screen, simulate_swipe) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.swipe() method with duration."""

        touch_device = self.user_input().create_touch_device(
            touch_screen_size=ui_custom_types.Size(width=4, height=5),
        )
        await touch_device.swipe(
            start_location=ui_custom_types.Coordinate(x=1, y=2),
            end_location=ui_custom_types.Coordinate(x=3, y=4),
            move_event_count=2,
            duration_ms=100,
        )
        simulate_swipe.assert_has_calls(
            [
                mock.call(
                    start_location=f_math.Vec(x=1, y=2),
                    end_location=f_math.Vec(x=3, y=4),
                    move_event_count=2,
                    duration=100 * 1000000,
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_keyboard",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_create_keyboard_device(self, register_keyboard) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.create_keyboard_device() method."""

        keyboard_device = self.user_input().create_keyboard_device()
        await keyboard_device.make_ready()

        self.fc_transport_obj.connect_device_proxy.assert_called_once_with(
            custom_types.FidlEndpoint(
                "/core/ui", "fuchsia.ui.test.input.Registry"
            )
        )
        register_keyboard.assert_called_once()

        self.assertIsNotNone(keyboard_device._keyboard_proxy)

    @mock.patch.object(
        f_test_input.KeyboardClient,
        "simulate_key_press",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_keyboard",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_key_press(self, unused_register_keyboard, simulate_key_press) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.key_press() method."""

        keyboard_device = self.user_input().create_keyboard_device()
        await keyboard_device.key_press(
            key_code=0xFFFF0002,  # Power
        )
        simulate_key_press.assert_has_calls(
            [
                mock.call(
                    key_code=0xFFFF0002,
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_mouse",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_create_mouse_device(self, register_mouse) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.create_mouse_device() method."""

        mouse_device = self.user_input().create_mouse_device()
        await mouse_device.make_ready()

        self.fc_transport_obj.connect_device_proxy.assert_called_once_with(
            custom_types.FidlEndpoint(
                "/core/ui", "fuchsia.ui.test.input.Registry"
            )
        )
        register_mouse.assert_called_once()

        self.assertIsNotNone(mouse_device._mouse_proxy)

    @mock.patch.object(
        f_test_input.MouseClient,
        "simulate_mouse_event",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_mouse",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_scroll(self, unused_register_mouse, simulate_mouse_event) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.scroll() method."""

        mouse_device = self.user_input().create_mouse_device()
        await mouse_device.scroll(
            scroll_v_detent=1,
            scroll_h_detent=0,
        )
        simulate_mouse_event.assert_has_calls(
            [
                mock.call(
                    scroll_v_detent=1,
                    scroll_h_detent=0,
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.MouseClient,
        "simulate_mouse_event",
        new_callable=mock.AsyncMock,
    )
    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_mouse",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_click(self, unused_register_mouse, simulate_mouse_event) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.click() method."""

        mouse_device = self.user_input().create_mouse_device()
        await mouse_device.click(
            button=0,
        )
        simulate_mouse_event.assert_has_calls(
            [
                mock.call(
                    movement_x=0,
                    movement_y=0,
                    pressed_buttons=[f_test_input.MouseButton.FIRST],
                    scroll_v_detent=0,
                    scroll_h_detent=0,
                ),
                mock.call(
                    movement_x=0,
                    movement_y=0,
                    pressed_buttons=[],
                    scroll_v_detent=0,
                    scroll_h_detent=0,
                ),
            ]
        )

        simulate_mouse_event.reset_mock()
        await mouse_device.click(
            button=1,
        )
        simulate_mouse_event.assert_has_calls(
            [
                mock.call(
                    movement_x=0,
                    movement_y=0,
                    pressed_buttons=[f_test_input.MouseButton.SECOND],
                    scroll_v_detent=0,
                    scroll_h_detent=0,
                ),
                mock.call(
                    movement_x=0,
                    movement_y=0,
                    pressed_buttons=[],
                    scroll_v_detent=0,
                    scroll_h_detent=0,
                ),
            ]
        )

    @mock.patch.object(
        f_test_input.RegistryClient,
        "register_mouse",
        new_callable=mock.AsyncMock,
        return_value=None,
    )
    async def test_click_invalid_button_raises_error(self, unused_register_mouse) -> None:  # type: ignore[no-untyped-def]
        """Test for UserInput.click() method with invalid button index."""
        mouse_device = self.user_input().create_mouse_device()
        with self.assertRaisesRegex(
            user_input_errors.UserInputError,
            "Unsupported mouse button index: 3",
        ):
            await mouse_device.click(button=3)


if __name__ == "__main__":
    unittest.main()
