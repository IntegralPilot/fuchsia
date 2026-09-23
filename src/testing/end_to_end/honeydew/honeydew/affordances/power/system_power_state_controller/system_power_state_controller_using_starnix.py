# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""SystemPowerStateController affordance implementation using startnix."""

import contextlib
import io
import logging
import re
import subprocess
import typing
from collections.abc import AsyncGenerator
from typing import Any

import fuchsia_inspect
from honeydew import affordances_capable, errors
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller as system_power_state_controller_interface,
)
from honeydew.affordances.starnix import starnix
from honeydew.transports.ffx import ffx as ffx_transport
from honeydew.typing import custom_types
from honeydew.utils import decorators

# TODO: b/354239403: This can not be done today, but probably should be done at
# some point:
# import fidl_fuchsia_power_observability as fobs
# Then, replace all labels appearing in constants.fidl with strings from there.


class _StarnixCmds:
    """Class to hold Starnix commands."""

    IDLE_SUSPEND: list[str] = [
        "echo -n mem > /sys/power/state",
    ]


class _FuchsiaCmds:
    """Class to hold Fuchsia commands."""

    SSH_PREFIX: list[str] = [
        "target",
        "ssh",
    ]

    @staticmethod
    def set_timer_command(duration: int) -> str:
        return f"hrtimer-ctl --id 2 --event {duration}"


_RESUME_DURATION_TOLERANCE_MILLISECONDS: float = 200


class _RegExPatterns:
    """Class to hold Regular Expression patterns."""

    SUSPEND_OPERATION: re.Pattern[str] = re.compile(
        r"\[(\d+?\.\d+?)\].*?\[system-activity-governor\].+?Suspending"
    )

    RESUME_OPERATION: re.Pattern[str] = re.compile(
        r"\[(\d+?\.\d+?)\].*?\[system-activity-governor\].+?Resuming.+?Ok"
    )

    HONEYDEW_SUSPEND_RESUME_START: re.Pattern[str] = re.compile(
        r"\[lacewing\].*?\[Host Time: (.*?)\].*?Performing.*?Suspend.*?followed by.*?Resume.*?operations"
    )

    HONEYDEW_SUSPEND_RESUME_END: re.Pattern[str] = re.compile(
        r"\[lacewing\].*?\[Host Time: (.*?)\].*?Completed.*?Suspend.*?followed by.*?Resume.*?operations.*?in (\d+?.\d+?) seconds"
    )

    SUSPEND_RESUME_PATTERNS: list[re.Pattern[str]] = [
        HONEYDEW_SUSPEND_RESUME_START,
        SUSPEND_OPERATION,
        RESUME_OPERATION,
        HONEYDEW_SUSPEND_RESUME_END,
    ]

    TIMER_STARTED: re.Pattern[str] = re.compile(r"Timer started")

    TIMER_ENDED: re.Pattern[str] = re.compile(r"Event trigged")


_LOGGER: logging.Logger = logging.getLogger(__name__)


class SystemPowerStateControllerUsingStarnix(
    system_power_state_controller_interface.SystemPowerStateController
):
    """Async SystemPowerStateController affordance implementation using sysfs.

    Args:
        device_name: Device name returned by `ffx target list`.
        ffx: interfaces.transports.FFX implementation.
        device_logger: FuchsiaDeviceLogger implementation.
        inspect: InspectCapableDevice implementation.
        starnix_affordance: Starnix implementation.

    Raises:
        errors.NotSupportedError: If Fuchsia device does not support Starnix.
    """

    def __init__(
        self,
        device_name: str,
        ffx: ffx_transport.FFX,
        device_logger: affordances_capable.FuchsiaDeviceLogger,
        inspect: affordances_capable.InspectCapableDevice,
        starnix_affordance: starnix.Starnix,
    ) -> None:
        self._device_name: str = device_name
        self._ffx: ffx_transport.FFX = ffx
        self._device_logger: affordances_capable.FuchsiaDeviceLogger = (
            device_logger
        )
        self._inspect: affordances_capable.InspectCapableDevice = inspect
        self._starnix = starnix_affordance

    # List all the public methods
    def verify_supported(self) -> None:
        # Already verified this during the init of `starnix.Starnix` affordance
        pass

    async def suspend_resume(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
        resume_mode: system_power_state_controller_interface.ResumeMode,
    ) -> None:
        """Perform suspend-resume operation on the device.

        This is a synchronous operation on the device and thus this call will
        hang until resume operation finishes.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.

        Raises:
            SystemPowerStateControllerError: In case of failure
            errors.NotSupportedError: If any of the suspend_state or resume_type
                is not yet supported
            ValueError: If any of the input args are not valid
        """
        self._validate_suspend_resume_method_args(
            suspend_state=suspend_state,
            resume_mode=resume_mode,
        )

        async with self._valid_suspend_resume_using_inspect(
            suspend_state=suspend_state,
            resume_mode=resume_mode,
        ):
            log_message: str = (
                f"Performing '{suspend_state}' followed by '{resume_mode}' "
                f"operations on '{self._device_name}'..."
            )
            _LOGGER.info(log_message)
            await self._device_logger.log_message_to_device(
                message=log_message,
                level=custom_types.LEVEL.INFO,
            )

            async with self._set_resume_mode(resume_mode=resume_mode):
                await self._suspend(suspend_state=suspend_state)

            log_message = (
                f"Completed '{suspend_state}' followed by '{resume_mode}' "
                f"operations on '{self._device_name}'."
            )
            await self._device_logger.log_message_to_device(
                message=log_message,
                level=custom_types.LEVEL.INFO,
            )
            _LOGGER.info(log_message)

    async def idle_suspend_timer_based_resume(
        self,
        duration: int,
        verify_duration: bool = True,
    ) -> None:
        """Perform idle-suspend and timer-based-resume operation on the device.

        Args:
            duration: Resume timer duration in seconds.
            verify_duration: If set to True, verifies suspend-resume operation completed with in the
                duration specified. If set to False, skips this verification. Default is True.

        Raises:
            SystemPowerStateControllerError: In case of failure
            ValueError: If any of the input args are not valid
        """
        await self.suspend_resume(
            suspend_state=system_power_state_controller_interface.IdleSuspend(),
            resume_mode=system_power_state_controller_interface.TimerResume(
                duration=duration,
                verify_duration=verify_duration,
            ),
        )

    # List all the private methods
    def _validate_suspend_resume_method_args(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
        resume_mode: system_power_state_controller_interface.ResumeMode,
    ) -> None:
        """Validate the input args of suspend_resume() method.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.

        Raises:
            errors.NotSupportedError: If any of the suspend_state or resume_type
                is not yet supported
            ValueError: If any of the input args are not valid
        """
        if not isinstance(
            suspend_state,
            (system_power_state_controller_interface.IdleSuspend,),
        ):
            raise errors.NotSupportedError(
                f"Suspending the device to '{suspend_state}' state is not yet "
                f"supported."
            )

        if not isinstance(
            resume_mode,
            (system_power_state_controller_interface.TimerResume,),
        ):
            raise errors.NotSupportedError(
                f"Resuming the device using '{resume_mode}' is not yet supported."
            )

    async def _suspend(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
    ) -> None:
        """Perform suspend operation on the device.

        This is a synchronous operation on the device and thus this call will
        hang until resume operation finishes.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.

        Raises:
            SystemPowerStateControllerError: In case of failure
        """
        _LOGGER.info(
            "Putting '%s' into '%s'",
            self._device_name,
            suspend_state,
        )

        if isinstance(
            suspend_state, system_power_state_controller_interface.IdleSuspend
        ):
            await self._perform_idle_suspend()

        _LOGGER.info(
            "'%s' has been resumed from '%s'",
            self._device_name,
            suspend_state,
        )

    async def _perform_idle_suspend(self) -> None:
        """Perform Idle mode suspend operation.

        Raises:
            SystemPowerStateControllerError: In case of failure.
        """
        try:
            await self._starnix.run_console_shell_cmd(
                cmd=_StarnixCmds.IDLE_SUSPEND,
            )
        except errors.HoneydewError as err:
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Failed to put {self._device_name} into idle-suspend mode"
            ) from err

    @contextlib.asynccontextmanager
    async def _set_resume_mode(
        self,
        resume_mode: system_power_state_controller_interface.ResumeMode,
    ) -> AsyncGenerator[None, None]:
        """Perform resume operation on the device.

        This is a synchronous operation on the device and thus call will hang
        until resume operation finishes. So we will be using a context
        manager which will start resume mode using subprocess, saves the proc
        and yields and when called again will wait for resume operation to be
        finished using the saved proc.

        Args:
            resume_mode: Information about how to resume the device.

        Raises:
            SystemPowerStateControllerError: In case of failure
            errors.HoneydewTimeoutError: If timer has not been started in 2 sec
        """
        _LOGGER.info(
            "Informing '%s' to resume using '%s'",
            self._device_name,
            resume_mode,
        )

        if isinstance(
            resume_mode, system_power_state_controller_interface.TimerResume
        ):
            proc: subprocess.Popen[str] = self._set_timer(resume_mode.duration)
            self._wait_for_timer_start(proc=proc)

        yield

        if isinstance(
            resume_mode, system_power_state_controller_interface.TimerResume
        ):
            self._wait_for_timer_end(proc=proc)

    def _set_timer(self, duration: int) -> subprocess.Popen[str]:
        """Sets the timer.

        Args:
            duration: Resume timer duration in seconds.

        Raises:
            SystemPowerStateControllerError: In case of failure.
        """
        try:
            _LOGGER.info(
                "Setting a timer for '%s sec' on '%s'",
                duration,
                self._device_name,
            )
            proc: subprocess.Popen[str] = self._ffx.popen(
                cmd=_FuchsiaCmds.SSH_PREFIX
                + [_FuchsiaCmds.set_timer_command(duration=duration)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            return proc
        except Exception as err:  # pylint: disable=broad-except
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Failed to set timer on {self._device_name}"
            ) from err

    @decorators.liveness_check
    def _wait_for_timer_start(self, proc: subprocess.Popen[str]) -> None:
        """Wait for the timer to start on the device.

        Args:
            proc: process used to set the timer.

        Raises:
            SystemPowerStateControllerError: Timer start failed.
        """
        std_out: typing.IO[str] | None = proc.stdout
        if not isinstance(std_out, io.TextIOWrapper):
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Failed to read hrtimer-ctl output on {self._device_name}"
            )
        _LOGGER.info("Waiting for the timer to start on %s", self._device_name)
        while True:
            try:
                line: str = std_out.readline()
                _LOGGER.debug(
                    "Line read from the hrtimer-ctl command output: %s",
                    line.strip(),
                )

                if _RegExPatterns.TIMER_STARTED.search(line):
                    _LOGGER.info(
                        "Timer has been started on %s", self._device_name
                    )
                    break
                elif line == "":  # End of output
                    raise system_power_state_controller_interface.SystemPowerStateControllerError(
                        "hrtimer-ctl completed without starting a timer"
                    )
            except Exception as err:  # pylint: disable=broad-except
                raise system_power_state_controller_interface.SystemPowerStateControllerError(
                    f"Timer has not been started on {self._device_name}"
                ) from err

    @decorators.liveness_check
    def _wait_for_timer_end(
        self,
        proc: subprocess.Popen[str],
    ) -> None:
        """Wait for the timer to end on the device.

        Args:
            proc: process used to set the timer.

        Raises:
            SystemPowerStateControllerError: Timer end failed.
        """
        output: str
        error: str
        output, error = proc.communicate()

        if proc.returncode != 0:
            message: str = (
                f"hrtimer-ctl returned a failure while waiting for the timer "
                f"to end. returncode={proc.returncode}"
            )
            if error:
                message = f"{message}, error='{error}'"
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                message
            )

        if _RegExPatterns.TIMER_ENDED.search(output):
            _LOGGER.info("Timer has been ended on %s", self._device_name)
        else:
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                "hrtimer-ctl completed without ending the timer"
            )

    @contextlib.asynccontextmanager
    async def _valid_suspend_resume_using_inspect(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
        resume_mode: system_power_state_controller_interface.ResumeMode,
    ) -> AsyncGenerator[None, None]:
        """Validate suspend-resume operation using inspect data.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.

        Raises:
            SystemPowerStateControllerError: In case of failure
        """
        suspend_resume_stats_before: dict[
            str, int
        ] = await self._get_suspend_stats_from_sag_inspect_data()

        suspend_resume_events_before: dict[
            str, dict[str, int]
        ] = await self._get_suspend_events_from_fsh_inspect_data()

        yield

        suspend_resume_stats_after: dict[
            str, int
        ] = await self._get_suspend_stats_from_sag_inspect_data()
        self._validate_using_sag_inspect_data(
            suspend_state,
            resume_mode,
            suspend_resume_stats_before,
            suspend_resume_stats_after,
        )

        suspend_resume_events_after: dict[
            str, dict[str, int]
        ] = await self._get_suspend_events_from_fsh_inspect_data()
        self._validate_using_fsh_inspect_data(
            suspend_state,
            resume_mode,
            suspend_resume_events_before,
            suspend_resume_events_after,
        )

    def _validate_using_sag_inspect_data(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
        resume_mode: system_power_state_controller_interface.ResumeMode,
        suspend_resume_stats_before: dict[str, int],
        suspend_resume_stats_after: dict[str, int],
    ) -> None:
        """Validate suspend-resume operation using SAG inspect data.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.
            suspend_resume_stats_before: Suspend-Resume stats before.
            suspend_resume_stats_after: Suspend-Resume stats after.

        Raises:
            SystemPowerStateControllerError: In case of failure
        """
        # "suspend_stats": {
        #     "fail_count": 0,
        #     "last_failed_error": 0,
        #     "last_time_in_suspend_ns": 3053743875,
        #     "last_time_in_suspend_operations": 188959,
        #     "success_count": 1
        # },

        if (
            suspend_resume_stats_after["success_count"]
            <= suspend_resume_stats_before["success_count"]
        ):
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Based on SAG inspect data, '{suspend_state}' followed "
                f"by '{resume_mode}' operation didn't succeed on "
                f"'{self._device_name}', expected "
                f"{suspend_resume_stats_before['success_count'] + 1}, "
                f"but found {suspend_resume_stats_after['success_count']}."
            )

        suspend_resume_duration_nano_sec: float = suspend_resume_stats_after[
            "last_time_in_suspend_ns"
        ]
        suspend_resume_duration_sec: float = round(
            suspend_resume_duration_nano_sec / 1e9, 4
        )

        self._validate_suspend_resume_duration_using_inspect_data(
            suspend_state=suspend_state,
            resume_mode=resume_mode,
            actual_suspend_resume_duration_nano_sec=suspend_resume_duration_nano_sec,
            inspect_data_src="SAG",
        )

        _LOGGER.info(
            "Successfully verified '%s' followed by '%s' operations on '%s' "
            "using SAG inspect data. Operations took '%s seconds' to complete.",
            suspend_state,
            resume_mode,
            self._device_name,
            suspend_resume_duration_sec,
        )

    def _validate_using_fsh_inspect_data(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
        resume_mode: system_power_state_controller_interface.ResumeMode,
        suspend_resume_events_before: dict[str, dict[str, int]],
        suspend_resume_events_after: dict[str, dict[str, int]],
    ) -> None:
        """Validate suspend-resume operation using FSH inspect data.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.
            suspend_resume_events_before: Suspend-Resume events before.
            suspend_resume_events_after: Suspend-Resume events after.

        Raises:
            SystemPowerStateControllerError: In case of failure
        """
        # suspend_resume_events_before:
        # {
        #     '0': {'attempted_at_ns': 75685149500},
        #     '1': {'resumed_at_ns': 76510325583},
        # }

        # suspend_resume_events_after:
        # {
        #     '0': {'attempted_at_ns': 75685149500},
        #     '1': {'resumed_at_ns': 76510325583},
        #     '2': {'attempted_at_ns': 109243654875},
        #     '3': {'resumed_at_ns': 109833015166}
        # }

        # current_suspend_resume_events:
        # {
        #     '2': {'attempted_at_ns': 109243654875},
        #     '3': {'resumed_at_ns': 109833015166}
        # }
        current_suspend_resume_events: dict[str, dict[str, int]] = {
            k: v
            for k, v in suspend_resume_events_after.items()
            if k not in suspend_resume_events_before
        }
        if (
            len(current_suspend_resume_events) < 2
            or len(current_suspend_resume_events) % 2 != 0
        ):
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Based on FSH inspect data, '{suspend_state}' followed "
                f"by '{resume_mode}' operation didn't succeed on "
                f"'{self._device_name}'. "
            )

        suspend_enter_timer: int | None = None
        suspend_exit_timer: int | None = None
        for k, v in current_suspend_resume_events.items():
            if "attempted_at_ns" in v:
                suspend_enter_timer = v["attempted_at_ns"]
            elif "resumed_at_ns" in v:
                suspend_exit_timer = v["resumed_at_ns"]
        if (
            suspend_enter_timer is None
            or suspend_exit_timer is None
            or suspend_exit_timer < suspend_enter_timer
        ):
            _LOGGER.info(
                "FSH inspect based suspend_resume_events associated with "
                "current suspend-resume operation: %s",
                current_suspend_resume_events,
            )
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Based on FSH inspect data, '{suspend_state}' followed "
                f"by '{resume_mode}' operation didn't succeed on "
                f"'{self._device_name}'. "
            )

        suspend_resume_duration_nano_sec: float = (
            suspend_exit_timer - suspend_enter_timer
        )
        suspend_resume_duration_sec: float = round(
            suspend_resume_duration_nano_sec / 1e9, 4
        )

        self._validate_suspend_resume_duration_using_inspect_data(
            suspend_state=suspend_state,
            resume_mode=resume_mode,
            actual_suspend_resume_duration_nano_sec=suspend_resume_duration_nano_sec,
            inspect_data_src="FSH",
        )

        _LOGGER.info(
            "Successfully verified '%s' followed by '%s' operations on '%s' "
            "using FSH inspect data. Operations took '%s seconds' to complete.",
            suspend_state,
            resume_mode,
            self._device_name,
            suspend_resume_duration_sec,
        )

    def _validate_suspend_resume_duration_using_inspect_data(
        self,
        suspend_state: system_power_state_controller_interface.SuspendState,
        resume_mode: system_power_state_controller_interface.ResumeMode,
        actual_suspend_resume_duration_nano_sec: float,
        inspect_data_src: str,
    ) -> None:
        """Validate suspend-resume duration received from inspect data.

        Args:
            suspend_state: Which state to suspend the Fuchsia device into.
            resume_mode: Information about how to resume the device.
            actual_suspend_resume_duration_nano_sec: Suspend-Resume duration.
            inspect_data_src: Inspect data source.

        Raises:
            SystemPowerStateControllerError: In case of failure
        """
        if not isinstance(
            resume_mode,
            (system_power_state_controller_interface.TimerResume,),
        ):
            return

        if resume_mode.verify_duration is False:
            return

        # It could take few milliseconds for the device to fully resume after the timer fires.
        resume_duration_tolerance_nano_sec: float = (
            _RESUME_DURATION_TOLERANCE_MILLISECONDS * 1e6
        )
        expected_suspend_resume_duration_with_tolerance_nano_sec: float = (
            resume_mode.duration * 1e9
        ) + resume_duration_tolerance_nano_sec

        if (
            actual_suspend_resume_duration_nano_sec
            > expected_suspend_resume_duration_with_tolerance_nano_sec
        ):
            actual_suspend_resume_duration_sec: float = round(
                actual_suspend_resume_duration_nano_sec / 1e9, 4
            )

            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Based on {inspect_data_src} inspect data, '{suspend_state}' followed by "
                f"'{resume_mode}' operation took "
                f"'{actual_suspend_resume_duration_sec} seconds' on "
                f"'{self._device_name}'. Expected it to not take more than "
                f"'{resume_mode.duration} seconds'.",
            )

    # pylint: disable=missing-raises-doc
    async def _get_suspend_stats_from_sag_inspect_data(self) -> dict[str, int]:
        """Returns suspend-resume stats by reading the SAG inspect data.

        Returns:
            suspend-resume stats.

        Raises:
            SystemPowerStateControllerError: Failed to read SAG inspect data.
        """
        try:
            inspect_data_collection: fuchsia_inspect.InspectDataCollection = (
                self._inspect.get_inspect_data(
                    selectors=["/bootstrap/system-activity-governor"],
                )
            )
            _LOGGER.debug(
                "SAG Inspect data associated with '%s' is: %s",
                self._device_name,
                inspect_data_collection,
            )

            if len(inspect_data_collection.data) == 0:
                raise errors.InspectError(
                    f"SAG inspect data associated with {self._device_name} is empty"
                )

            sag_inspect_data: fuchsia_inspect.InspectData = (
                inspect_data_collection.data[0]
            )

            if sag_inspect_data.payload is None:
                raise errors.InspectError(
                    f"SAG inspect data associated with {self._device_name} does "
                    f"not have a valid payload"
                )

            sag_inspect_root_data: dict[str, Any] = sag_inspect_data.payload[
                "root"
            ]
            return sag_inspect_root_data["suspend_stats"]
        except errors.InspectError as err:
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Failed to read SAG inspect data from {self._device_name}"
            ) from err

    async def _get_suspend_events_from_fsh_inspect_data(
        self,
    ) -> dict[str, dict[str, int]]:
        """Returns suspend-resume events by reading the FSH inspect data.

        Returns:
            suspend-resume events.

        Raises:
            SystemPowerStateControllerError: Failed to read FSH inspect data.
        """
        try:
            inspect_data_collection: fuchsia_inspect.InspectDataCollection = (
                self._inspect.get_inspect_data(
                    selectors=[
                        "bootstrap/boot-drivers*:[name=generic-suspend]root"
                    ],
                )
            )
            _LOGGER.debug(
                "FSH (Fuchsia Suspend HAL) inspect data associated with '%s' is: %s",
                self._device_name,
                inspect_data_collection,
            )

            if len(inspect_data_collection.data) == 0:
                raise errors.InspectError(
                    f"FSH inspect data associated with {self._device_name} is empty"
                )

            fsh_inspect_data: fuchsia_inspect.InspectData = (
                inspect_data_collection.data[0]
            )

            if fsh_inspect_data.payload is None:
                raise errors.InspectError(
                    f"FSH inspect data associated with {self._device_name} does "
                    f"not have a valid payload"
                )

            fsh_inspect_root_data: dict[str, Any] = fsh_inspect_data.payload[
                "root"
            ]
            suspend_events: dict[str, dict[str, int]] = fsh_inspect_root_data[
                "suspend_events"
            ]

            return suspend_events
        except errors.InspectError as err:
            raise system_power_state_controller_interface.SystemPowerStateControllerError(
                f"Failed to read FSH inspect data from {self._device_name}"
            ) from err
