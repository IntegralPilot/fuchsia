# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for power switch hardware."""

from __future__ import annotations

import abc
import logging

_LOGGER: logging.Logger = logging.getLogger(__name__)


class PowerSwitchError(Exception):
    """Exception class for PowerSwitch relates error."""

    def __init__(self, msg: str | Exception) -> None:
        """Inits PowerSwitchDmcError with 'msg' (an error message string).

        Args:
            msg: an error message string or an Exception instance.

        Note: Additionally, logs 'msg' to debug log level file.
        """
        super().__init__(msg)
        _LOGGER.debug(repr(self), exc_info=True)


class PowerSwitch(abc.ABC):
    """Abstract base class for power switch hardware."""

    # List all the public methods
    @abc.abstractmethod
    def power_off(self, outlet: int | None = None) -> None:
        """Turns off the power at the specified outlet on the power switch.

        Args:
            outlet: outlet on which we need to perform this operation.

        Raises:
            PowerSwitchError: In case of failure.
        """

    @abc.abstractmethod
    def power_on(self, outlet: int | None = None) -> None:
        """Turns on the power at the specified outlet on the power switch.

        Args:
            outlet: outlet on which we need to perform this operation.

        Raises:
            PowerSwitchError: In case of failure.
        """
