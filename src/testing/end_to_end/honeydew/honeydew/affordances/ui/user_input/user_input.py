# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for UserInput affordance."""

import abc
from typing import Any

from honeydew.affordances.ui.user_input import types

DEFAULTS: dict[str, Any] = {
    "TOUCH_SCREEN_SIZE": types.Size(width=1000, height=1000),
    "TAP_EVENT_COUNT": 1,
    "TAP_DURATION_MS": 300,
    "SWIPE_DURATION_MS": 0,
    "ONE_TAP_DURATION_MS": 0,
    "MOUSE_BUTTON": 0,
}


class TouchDevice(abc.ABC):
    """Abstract base class for an async UserInput Touch."""

    @abc.abstractmethod
    async def tap(
        self,
        location: types.Coordinate,
        tap_event_count: int = DEFAULTS["TAP_EVENT_COUNT"],
        duration_ms: int = DEFAULTS["TAP_DURATION_MS"],
        duration_of_one_tap_ms: int = DEFAULTS["ONE_TAP_DURATION_MS"],
    ) -> None:
        """Instantiates Taps at coordinates (x, y) for a touchscreen with
           default or custom width, height, duration, and tap event counts.

        Args:
            location: tap location in X, Y axis coordinate.

            tap_event_count: Number of tap events to send (`duration` is
                divided over the tap events), defaults to 1.

            duration_ms: Duration of the event(s) in milliseconds, defaults to
                300.

            duration_of_one_tap_ms: Duration of 1 event(s) in milliseconds,
                defaults to 0.

        Raises:
            UserInputError: if failed tap operation.
        """

    @abc.abstractmethod
    async def swipe(
        self,
        start_location: types.Coordinate,
        end_location: types.Coordinate,
        move_event_count: int,
        duration_ms: int = DEFAULTS["SWIPE_DURATION_MS"],
    ) -> None:
        """Instantiates a swipe event sequence that starts at `start_location` and ends at
           `end_location`, with a total number of move events equal to `move_event_count`.

           Events are injected with no explicit delay in between.

        Args:
            start_location: swipe start location in X, Y axis coordinate.

            end_location: swipe end location in X, Y axis coordinate.

            move_event_count: Number of move events.

            duration_ms: Duration of the swipe gesture in milliseconds, defaults to 0.

        Raises:
            UserInputError: if failed swipe operation.
        """


class KeyboardDevice(abc.ABC):
    """Abstract base class for an async UserInput Keyboard or Button."""

    @abc.abstractmethod
    async def key_press(
        self,
        key_code: int,
    ) -> None:
        """Instantiates key press includes down and up.

        Args:
            key_code: key code you can find in fuchsia.input.Key

        Raises:
            UserInputError: if failed key press operation.
        """


class AsyncMouseDevice(abc.ABC):
    """Abstract base class for an async UserInput Mouse."""

    @abc.abstractmethod
    async def scroll(
        self,
        scroll_v_detent: int = 0,
        scroll_h_detent: int = 0,
    ) -> None:
        """Instantiates a scroll event.

        Args:
            scroll_v_detent: Relative vertical scrolling displacement by detent.
            scroll_h_detent: Relative horizontal scrolling displacement by detent.

        Raises:
            UserInputError: if failed scroll operation.
        """

    @abc.abstractmethod
    async def click(
        self,
        button: int = DEFAULTS["MOUSE_BUTTON"],
    ) -> None:
        """Instantiates a click event (button down and up).

        Args:
            button: mouse button id, defaults to 0 (FIRST).

        Raises:
            UserInputError: if failed click operation.
        """


class UserInput(abc.ABC):
    """Abstract base class for an async UserInput affordance."""

    @abc.abstractmethod
    def create_touch_device(
        self,
        touch_screen_size: types.Size = DEFAULTS["TOUCH_SCREEN_SIZE"],
    ) -> TouchDevice:
        """Create a virtual touch device for testing touch input.

        Args:
            touch_screen_size: resolution of the touch screen, defaults to
                1000 x 1000.

        Returns:
            TouchDevice object.

        Raises:
            UserInputError: if failed to create virtual touch device.
        """

    @abc.abstractmethod
    def create_keyboard_device(self) -> KeyboardDevice:
        """Create a virtual keyboard device for testing keyboard input.

        Returns:
            KeyboardDevice object.

        Raises:
            UserInputError: if failed to create virtual keyboard device.
        """

    @abc.abstractmethod
    def create_mouse_device(self) -> AsyncMouseDevice:
        """Create a virtual mouse device for testing mouse input.

        Returns:
            AsyncMouseDevice object.

        Raises:
            UserInputError: if failed to create virtual mouse device.
        """
