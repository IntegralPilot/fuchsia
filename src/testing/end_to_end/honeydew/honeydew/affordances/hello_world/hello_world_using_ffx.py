# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""HelloWorld affordance implementation using FFX."""

from honeydew import errors
from honeydew.affordances.hello_world import errors as hello_world_errors
from honeydew.affordances.hello_world import hello_world
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx as ffx_transport


class HelloWorldUsingFfx(hello_world.HelloWorld):
    """HelloWorld affordance implementation using FFX."""

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
    ) -> None:
        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx

        self.verify_supported()

    def verify_supported(self) -> None:
        """Verifies that HelloWorld affordance is supported by the Fuchsia device.

        This method should be called in every affordance implementation's `__init__()` so that if an
        affordance is used on a Fuchsia device that does not support it, it will raise
        NotSupportedError.

        Raises:
            NotSupportedError: If affordance is not supported.
        """
        try:
            self._ffx.check_connection()
        except ffx_errors.FfxConnectionError as err:
            raise errors.NotSupportedError(
                f"HelloWorld affordance is not supported by {self._device_name}"
            ) from err

    def greeting(self, name: str | None = None) -> str:
        """Returns a string that contains greeting's message.

        Returns:
            greeting's message str

        Raises:
            HelloWorldAffordanceError: Failed to construct greeting's message.
        """
        target_name: str = self._ffx.get_target_name()

        if name and name.lower() == target_name.lower():
            raise hello_world_errors.HelloWorldAffordanceError(
                f"Invalid value passed in name argument: {name}"
            )

        return f"Hello, {name}!" if name else f"Hello, {target_name}!"
