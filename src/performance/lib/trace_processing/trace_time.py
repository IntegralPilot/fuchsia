# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Nanonsecond-resolution time values for trace models."""

import dataclasses
from typing import Any, Self, overload


class TimeDelta:
    """Represents the difference between two points in time, with nanosecond
    precision.

    Unlike datetime.timedelta from the standard library, the delta supports
    nanosecond resolution and stores the time compactly as a series of
    nanosecond ticks.
    """

    def __init__(self, nanoseconds: int = 0) -> None:
        self._delta: int = nanoseconds

    @classmethod
    def zero(cls) -> Self:
        return cls(0)

    @classmethod
    def from_nanoseconds(cls, nanoseconds: float) -> Self:
        return cls(int(nanoseconds))

    @classmethod
    def from_microseconds(cls, microseconds: float) -> Self:
        return cls(int(microseconds * 1000))

    @classmethod
    def from_milliseconds(cls, milliseconds: float) -> Self:
        return cls(int(milliseconds * 1000 * 1000))

    @classmethod
    def from_seconds(cls, seconds: float) -> Self:
        return cls(int(seconds * 1000 * 1000 * 1000))

    def to_nanoseconds(self) -> int:
        return self._delta

    def to_microseconds(self) -> int:
        return self._delta // 1000

    def to_milliseconds(self) -> int:
        return self._delta // (1000 * 1000)

    def to_seconds(self) -> int:
        return self._delta // (1000 * 1000 * 1000)

    def to_nanoseconds_f(self) -> float:
        return float(self._delta)

    def to_microseconds_f(self) -> float:
        return float(self._delta) / 1000

    def to_milliseconds_f(self) -> float:
        return float(self._delta) / (1000 * 1000)

    def to_seconds_f(self) -> float:
        return float(self._delta) / (1000 * 1000 * 1000)

    def __add__(self, other: Any) -> "TimeDelta":
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return TimeDelta(self._delta + other._delta)

    def __sub__(self, other: Any) -> "TimeDelta":
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return TimeDelta(self._delta - other._delta)

    def __neg__(self) -> "TimeDelta":
        return TimeDelta(-self._delta)

    def __mul__(self, factor: int) -> "TimeDelta":
        return TimeDelta(self._delta * factor)

    def __truediv__(self, other: Any) -> float:
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return self._delta / other._delta

    def __lt__(self, other: Any) -> bool:
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return self._delta < other._delta

    def __gt__(self, other: Any) -> bool:
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return self._delta > other._delta

    def __le__(self, other: Any) -> bool:
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return self._delta <= other._delta

    def __ge__(self, other: Any) -> bool:
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return self._delta >= other._delta

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, TimeDelta):
            return NotImplemented
        return self._delta == other._delta

    def __hash__(self) -> int:
        return hash(self._delta)

    def __repr__(self) -> str:
        return f"{self._delta}ns"


class TimePoint:
    """Represents a point in time as an integer number of nanoseconds elapsed
    since an arbitrary point in the past."""

    def __init__(self, nanoseconds: int = 0) -> None:
        self._ticks = nanoseconds

    @classmethod
    def zero(cls) -> Self:
        return cls(0)

    @classmethod
    def from_epoch_delta(cls, time_delta: TimeDelta) -> Self:
        return cls(time_delta.to_nanoseconds())

    def to_epoch_delta(self) -> TimeDelta:
        return TimeDelta.from_nanoseconds(self._ticks)

    def __add__(self, time_delta: TimeDelta) -> "TimePoint":
        return TimePoint(self._ticks + time_delta.to_nanoseconds())

    @overload
    def __sub__(self, other: "TimePoint") -> TimeDelta:
        pass

    @overload
    def __sub__(self, other: TimeDelta) -> "TimePoint":
        pass

    def __sub__(self, other: Any) -> "TimeDelta | TimePoint":
        if isinstance(other, TimePoint):
            return TimeDelta.from_nanoseconds(self._ticks - other._ticks)
        if isinstance(other, TimeDelta):
            return self + (-other)
        return NotImplemented

    def __lt__(self, other: Any) -> bool:
        if not isinstance(other, TimePoint):
            return NotImplemented
        return self._ticks < other._ticks

    def __gt__(self, other: Any) -> bool:
        if not isinstance(other, TimePoint):
            return NotImplemented
        return self._ticks > other._ticks

    def __le__(self, other: Any) -> bool:
        if not isinstance(other, TimePoint):
            return NotImplemented
        return self._ticks <= other._ticks

    def __ge__(self, other: Any) -> bool:
        if not isinstance(other, TimePoint):
            return NotImplemented
        return self._ticks >= other._ticks

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, TimePoint):
            return NotImplemented
        return self._ticks == other._ticks

    def __hash__(self) -> int:
        return hash(self._ticks)

    def __repr__(self) -> str:
        return f"{self.to_epoch_delta()}"


@dataclasses.dataclass(frozen=True)
class Window:
    """A pair of TimePoints during which the device was in an interesting state.

    Args:
      start: a TimePoint by which the device was in the desired state.
      end: a TimePoint after which the device exited the relevant state.
    """

    start: TimePoint
    end: TimePoint

    def __contains__(self, b: TimePoint) -> bool:
        return self.start <= b < self.end
