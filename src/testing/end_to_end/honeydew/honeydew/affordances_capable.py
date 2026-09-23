# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains Abstract Base Classes for all affordances capable devices."""

import abc
from collections.abc import Awaitable, Callable

import fuchsia_inspect
from honeydew.typing import custom_types


class RebootCapableDevice(abc.ABC):
    """Abstract base class to be implemented by a device which supports the
    reboot operation."""

    @abc.abstractmethod
    async def reboot(self) -> None:
        """Soft reboot the device."""

    @abc.abstractmethod
    async def on_device_boot(self) -> None:
        """Take actions after the device is rebooted."""

    @abc.abstractmethod
    def register_for_on_device_boot(
        self, fn: Callable[[], None] | Callable[[], Awaitable[None]]
    ) -> None:
        """Register a function that will be called in `on_device_boot()`.

        Args:
            fn: Function that need to be called after FuchsiaDevice boot up.
        """

    @abc.abstractmethod
    def wait_for_offline(self) -> None:
        """Wait for Fuchsia device to go offline."""

    @abc.abstractmethod
    async def wait_for_online(self) -> None:
        """Wait for Fuchsia device to go online."""


class FuchsiaDeviceLogger(abc.ABC):
    """Abstract base class which contains methods for logging message to fuchsia
    device."""

    @abc.abstractmethod
    async def log_message_to_device(
        self, message: str, level: custom_types.LEVEL
    ) -> None:
        """Log message to fuchsia device at specified level.

        Args:
            message: Message that need to logged.
            level: Log message level.
        """


class FuchsiaDeviceClose(abc.ABC):
    """Abstract base class which contains methods that let you register to run any custom logic
    during device cleanup."""

    @abc.abstractmethod
    def register_for_on_device_close(
        self, fn: Callable[[], None] | Callable[[], Awaitable[None]]
    ) -> None:
        """Register a function that will be called during device clean up in `close()`.

        Args:
            fn: Function that need to be called during FuchsiaDevice cleanup.
        """


class FuchsiaDeviceIpChange(abc.ABC):
    """Abstract base class which contains methods that let you register to run any custom logic
    when device IP address is changed."""

    @abc.abstractmethod
    def register_for_on_device_ip_change(
        self,
        fn: Callable[[custom_types.IpPort], None]
        | Callable[[custom_types.IpPort], Awaitable[None]],
    ) -> None:
        """Register a function that will be called when an IP address is changed.

        Args:
            fn: Function that need to be called when an IP address is changed.
        """


class InspectCapableDevice(abc.ABC):
    """Abstract base class to be implemented by a device which supports the
    inspect operation."""

    @abc.abstractmethod
    def get_inspect_data(
        self,
        selectors: list[str] | None = None,
        monikers: list[str] | None = None,
    ) -> fuchsia_inspect.InspectDataCollection:
        """Return the inspect data associated with the given selectors and
        monikers.

        Args:
            selectors: selectors to be queried.
            monikers: component monikers.

        Note: If both `selectors` and `monikers` lists are empty, inspect data
        for the whole system will be returned.

        Returns:
            Inspect data collection

        Raises:
            InspectError: Failed to return inspect data.
        """
