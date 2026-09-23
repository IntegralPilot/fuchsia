# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A more ergonomic Deadline type.

# Why not `datetime`?

The `datetime` module has a number of footguns that make it unsuitable for use
as a deadline type in tests:

1. It's easy to create a timezone-naive datetime, and creating timezone-aware
   datetimes is verbose.
2. `datetime` does not have a built-in way to represent an infinite deadline.
   `datetime.min` and `datetime.max` exist, but don't behave correctly with
   respect to arithmetic.
3. Infinite deadlines tend to be represented by `None`, which can be confused
   with a "default" deadline.
"""

from __future__ import annotations

import logging
from datetime import datetime, timedelta, timezone

from honeydew import errors

_LOGGER: logging.Logger = logging.getLogger(__name__)


class Deadline:
    """Holds a deadline timestamp."""

    _deadline: datetime

    def __init__(self, deadline: datetime) -> None:
        """Creates a Deadline specifying start_time and deadline manually."""
        assert deadline.tzinfo is not None, "Deadline must be timezone-aware"
        assert (
            deadline.tzinfo == timezone.utc
        ), f"Deadline must be in UTC timezone; found {deadline.tzinfo}"
        self._deadline = deadline

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Deadline):
            return NotImplemented
        return self._deadline == other._deadline

    @staticmethod
    def from_timeout(duration: timedelta) -> Deadline:
        """Creates a Deadline instance based on a duration and the current timestamp"""
        return Deadline(datetime.now(timezone.utc) + duration)

    @staticmethod
    def infinite_past() -> Deadline:
        """Creates a deadline that has already passed."""
        return Deadline(datetime.min.replace(tzinfo=timezone.utc))

    @staticmethod
    def infinite() -> Deadline:
        """Creates a deadline that will never pass."""
        return Deadline(datetime.max.replace(tzinfo=timezone.utc))

    def subdeadline_with_timeout(self, duration: timedelta) -> Deadline:
        """Like from_timeout, but the result will be no later than `self`."""
        if duration < timedelta(seconds=0):
            _LOGGER.warning("Timeout duration is negative, using 0")
            return Deadline(datetime.now(timezone.utc))

        if self._deadline == datetime.min.replace(tzinfo=timezone.utc):
            return self
        if self._deadline == datetime.max.replace(tzinfo=timezone.utc):
            return Deadline.from_timeout(duration)

        return Deadline(
            min(self._deadline, datetime.now(timezone.utc) + duration)
        )

    def subdeadline_with_grace_period(self, duration: timedelta) -> Deadline:
        """Returns a subdeadline that expires `duration` earlier than `self`."""
        if duration < timedelta(seconds=0):
            _LOGGER.warning("Grace period is negative, using 0")
            return self

        if self._deadline == datetime.max.replace(tzinfo=timezone.utc):
            return self
        if self._deadline == datetime.min.replace(tzinfo=timezone.utc):
            return self

        return Deadline(self._deadline - duration)

    def check(self) -> None:
        """Raises an exception if the deadline has passed."""
        if self.is_due():
            raise errors.HoneydewTimeoutError("Deadline has passed")

    def check_still_have(self, min_remaining: timedelta) -> None:
        """Raises an exception if the deadline has passed or will pass within min_remaining.

        Args:
            min_remaining: The minimum amount of time remaining until the
                deadline. If the remaining time is less than this, the
                deadline is considered due.
        """
        if self.is_due_before(min_remaining):
            raise errors.HoneydewTimeoutError(
                f"Deadline does not have the required {min_remaining} remaining"
            )

    def is_due(self) -> bool:
        """Returns True if the deadline has passed, False otherwise."""
        remaining = self.remaining_duration()
        return remaining is not None and remaining <= timedelta(seconds=0)

    def is_due_before(self, min_remaining: timedelta) -> bool:
        """Returns True if the deadline has passed or will pass within min_remaining.

        Args:
            min_remaining: The minimum amount of time remaining until the
                deadline. If the remaining time is less than this, the
                deadline is considered due.
        """
        remaining = self.remaining_duration()
        return remaining is not None and remaining <= min_remaining

    def remaining_duration(self) -> timedelta | None:
        """Returns the duration remaining until the deadline.

        Returns None if the deadline is infinite.
        """
        if self._deadline == datetime.max.replace(tzinfo=timezone.utc):
            return None
        return self._deadline - datetime.now(timezone.utc)

    def remaining_seconds(self) -> float | None:
        """Returns the number of seconds remaining until the deadline.

        Returns None if the deadline is in the infinite future.
        """
        if (remaining := self.remaining_duration()) is None:
            return None
        return remaining.total_seconds()

    def utc_datetime(self) -> datetime:
        """Returns the datetime of the deadline with UTC as the timezone.

        In general, prefer `check`, `is_due`, or `remaining_duration` over
        using this method.
        """
        return self._deadline

    def __str__(self) -> str:
        if self._deadline == datetime.min.replace(tzinfo=timezone.utc):
            return "Deadline(infinite_past)"
        if self._deadline == datetime.max.replace(tzinfo=timezone.utc):
            return "Deadline(infinite)"
        return f"Deadline(remaining={self.remaining_duration()}, due_at={self._deadline})"
