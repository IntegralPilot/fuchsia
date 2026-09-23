# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for real time clock (RTC) affordance."""

import abc
import datetime


class Rtc(abc.ABC):
    """Abstract base class for an async RTC affordance."""

    @abc.abstractmethod
    async def get(self) -> datetime.datetime:
        """Read time from the RTC.

        Returns:
            A datetime.datetime instance corresponding to the read time.

        Raises:
            HoneydewRtcError: Upon FIDL transaction failure.
        """

    @abc.abstractmethod
    async def set(self, time: datetime.datetime) -> None:
        """Set the time on the RTC.

        Args:
            time: The time to set in the RTC. Sub-seconds will be ignored.

        Raises:
            HoneydewRtcError: Upon FIDL transaction failure.
        """
