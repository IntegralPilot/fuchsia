# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for SystemPowerStateController affordance."""

import abc
from dataclasses import dataclass

from honeydew import errors


@dataclass(frozen=True)
class SuspendState(abc.ABC):
    """Abstract base class for different suspend states"""


@dataclass(frozen=True)
class IdleSuspend(SuspendState):
    """Idle suspend mode"""

    def __str__(self) -> str:
        return "IdleSuspend"


@dataclass(frozen=True)
class ResumeMode(abc.ABC):
    """Abstract base class for different resume modes"""


@dataclass(frozen=True)
class TimerResume(ResumeMode):
    """Resume after the given duration

    Args:
        duration: Timer duration in sec
        verify_duration: If set to True, verifies suspend-resume operation completed with in the
            duration specified. If set to False, skips this verification. Default is True.
    """

    duration: int
    verify_duration: bool = True

    def __str__(self) -> str:
        return f"TimerResume after {self.duration}sec"


@dataclass(frozen=True)
class ButtonPressResume(ResumeMode):
    """Resumes only on the button press"""

    def __str__(self) -> str:
        return "ButtonPressResume"


class SystemPowerStateControllerError(errors.HoneydewError):
    """Exception to be raised by SystemPowerStateController affordance."""


class SystemPowerStateController(abc.ABC):
    """Abstract base class for an async SystemPowerStateController affordance."""

    @abc.abstractmethod
    async def suspend_resume(
        self,
        suspend_state: SuspendState,
        resume_mode: ResumeMode,
    ) -> None:
        """Perform suspend-resume operation on the device.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.

        Raises:
            SystemPowerStateControllerError: In case of failure
            errors.NotSupportedError: If any of the suspend_state or resume_type
                is not yet supported
        """

    @abc.abstractmethod
    async def idle_suspend_timer_based_resume(
        self,
        duration: int,
        verify_duration: bool = True,
    ) -> None:
        """Perform idle-suspend and timer-based-resume operation on the device.

        Args:
            duration: Resume timer duration in seconds.
            verify_duration: If set to True, verifies suspend-resume operation completed within the
                duration specified. If set to False, skips this verification. Default is True.

        Raises:
            SystemPowerStateControllerError: In case of failure
        """
