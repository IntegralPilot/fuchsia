# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""ABC with methods for Host-(Fuchsia)Target interactions via Serial port."""

import abc


class Serial(abc.ABC):
    """ABC with methods for Host-(Fuchsia)Target interactions via Serial port."""

    @abc.abstractmethod
    def read(
        self,
        size: int = 512,
    ) -> str:
        """Reads bytes directly from the serial port and decodes them as a utf-8 string.

        This method makes no guarantees around timing of the serial output being read.
        For example, the output of a serial command from `send` is not guaranteed to be
        immediately available in the output of `read`.

        Args:
            size: The number of bytes to read.

        Returns:
            The bytes decoded into a utf-8 string.

        Raises:
            SerialError: In case of failure.
        """

    @abc.abstractmethod
    def send(
        self,
        cmd: str,
    ) -> None:
        """Send command over serial port and immediately returns without any
        further checking if it ran successfully or not.

        Args:
            cmd: Command to run over serial port.

        Raises:
            SerialError: In case of failure.
        """
