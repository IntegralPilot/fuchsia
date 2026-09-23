# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.affordances.starnix.system_power_state_controller.py."""

import io
import subprocess
import unittest
from collections.abc import Callable
from copy import deepcopy
from typing import Any, cast
from unittest import mock

import fuchsia_inspect
from honeydew import affordances_capable, errors
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller as system_power_state_controller_interface,
)
from honeydew.affordances.power.system_power_state_controller import (
    system_power_state_controller_using_starnix,
)
from honeydew.affordances.starnix import errors as starnix_errors
from honeydew.affordances.starnix import starnix
from honeydew.transports.ffx import ffx as ffx_transport
from parameterized import param, parameterized

# TODO: b/354239403: This can not be done today, but probably should be done at
# some point:
# import fidl_fuchsia_power_observability as fobs
# Then, replace all labels appearing in constants.fidl with strings from there.

_INPUT_ARGS: dict[str, object] = {
    "device_name": "fuchsia-emulator",
}

_HRTIMER_CTL_OUTPUT: list[str] = [
    "Executing on device /dev/class/hrtimer/4515c6a8",
    "Setting event...",
    "Starting timer...",
    "Timer started",
    "Waiting on event...",
    "Event trigged",
]

_HRTIMER_CTL_OUTPUT_NO_TIMER_START: list[str] = [
    "Executing on device /dev/class/hrtimer/4515c6a8",
    "Setting event...",
    "",
]

_HRTIMER_CTL_OUTPUT_NO_TIMER_END: list[str] = [
    "Executing on device /dev/class/hrtimer/4515c6a8",
    "Setting event...",
    "Starting timer...",
    "Timer started",
    "Waiting on event...",
]

_HRTIMER_CTL_OUTPUT_NO_TIMER_START_FOR_TIMEOUT: str = (
    "Executing on device /dev/class/hrtimer/4515c6a8"
)


_SAG_INSPECT_DATA_BEFORE: list[dict[str, Any]] = [
    {
        "data_source": "Inspect",
        "metadata": {
            "component_url": "fuchsia-boot:///system-activity-governor#meta/system-activity-governor.cm",
            "timestamp": 372140515750,
        },
        "moniker": "bootstrap/system-activity-governor",
        "payload": {
            "root": {
                "booting": False,
                "power_elements": {
                    "execution_resume_latency": {
                        "resume_latencies": [0],
                        "resume_latency": 0,
                        "power_level": 0,
                    },
                    "application_activity": {"power_level": 1},
                    "execution_state": {"power_level": 2},
                },
                "suspend_events": {},
                "suspend_stats": {
                    "success_count": 0,
                    "fail_count": 0,
                    "last_failed_error": 0,
                    "last_time_in_suspend_ns": -1,
                    "last_time_in_suspend_operations": -1,
                },
                "fuchsia.inspect.Health": {
                    "start_timestamp_nanos": 892116500,
                    "status": "OK",
                },
            }
        },
        "version": 1,
    }
]

_SAG_SUSPEND_STATS_AFTER: dict[str, int] = {
    "fail_count": 0,
    "last_failed_error": 0,
    "last_time_in_suspend_ns": 3053743875,
    "last_time_in_suspend_operations": 188959,
    "success_count": 1,
}
_SAG_INSPECT_DATA_AFTER: list[dict[str, Any]] = deepcopy(
    _SAG_INSPECT_DATA_BEFORE
)
_SAG_INSPECT_DATA_AFTER[0]["payload"]["root"][
    "suspend_stats"
] = _SAG_SUSPEND_STATS_AFTER

_FSH_INSPECT_DATA_BEFORE: list[dict[str, Any]] = [
    {
        "data_source": "Inspect",
        "metadata": {
            "component_url": "fuchsia-boot:///generic-suspend#meta/generic-suspend.cm",
            "timestamp": 372140515750,
        },
        "moniker": "'bootstrap/boot-drivers:suspend'",
        "name": "generic-suspend",
        "payload": {"root": {"suspend_events": {}}},
        "version": 1,
    }
]

_FSH_INSPECT_DATA_AFTER: list[dict[str, Any]] = deepcopy(
    _FSH_INSPECT_DATA_BEFORE
)
_FSH_INSPECT_DATA_AFTER[0]["payload"]["root"]["suspend_events"] = {
    "0": {"attempted_at_ns": 73886828041},
    "1": {"resumed_at_ns": 75687395083},
}

SUSPEND_RESUME_EVENTS_AFTER_FAIL_2: dict[str, dict[str, int]] = {
    "0": {"attempted_at_ns": 75687395083},
    "1": {"resumed_at_ns": 73886828041},
}

SUSPEND_RESUME_EVENTS_AFTER_FAIL_3: dict[str, dict[str, int]] = {
    "0": {"attempted_at_ns": 73886828041},
    "1": {"resumed_at_ns": 79886828041},
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_obj: param
) -> str:
    """Custom test name function method."""
    test_func_name: str = cast(Any, testcase_func).__name__
    test_label: str = parameterized.to_safe_name(param_obj.kwargs["label"])
    return f"{test_func_name}_with_{test_label}"


# pylint: disable=protected-access
class SystemPowerStateControllerStarnixTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.affordances.power.system_power_state_controller.system_power_state_controller_using_starnix.py."""

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()

        self.mock_ffx = mock.MagicMock(spec=ffx_transport.FFX)
        self.mock_device_logger = mock.AsyncMock(
            spec=affordances_capable.FuchsiaDeviceLogger
        )
        self.mock_inspect = mock.MagicMock(
            spec=affordances_capable.InspectCapableDevice
        )
        self.mock_starnix = mock.AsyncMock(spec=starnix.Starnix)

        self.system_power_state_controller_using_starnix_obj = system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix(
            ffx=self.mock_ffx,
            device_logger=self.mock_device_logger,
            inspect=self.mock_inspect,
            device_name=str(_INPUT_ARGS["device_name"]),
            starnix_affordance=self.mock_starnix,
        )

    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "suspend_resume",
        autospec=True,
    )
    async def test_idle_suspend_timer_based_resume(
        self, mock_suspend_resume: mock.AsyncMock
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.idle_suspend_timer_based_resume()"""

        await self.system_power_state_controller_using_starnix_obj.idle_suspend_timer_based_resume(
            duration=3,
        )
        mock_suspend_resume.assert_called_once_with(
            mock.ANY,
            suspend_state=system_power_state_controller_interface.IdleSuspend(),
            resume_mode=system_power_state_controller_interface.TimerResume(
                duration=3
            ),
        )

    async def test_suspend_resume_with_not_supported_suspend_mode(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.suspend_resume() raising
        NotSupportedError for invalid suspend operation."""
        with self.assertRaises(errors.NotSupportedError):
            await self.system_power_state_controller_using_starnix_obj.suspend_resume(
                suspend_state=cast(Any, "invalid"),
                resume_mode=system_power_state_controller_interface.TimerResume(
                    duration=3
                ),
            )

    async def test_suspend_resume_with_not_supported_resume_mode(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.suspend_resume() raising
        NotSupportedError for unsupported resume operation."""
        with self.assertRaises(errors.NotSupportedError):
            await self.system_power_state_controller_using_starnix_obj.suspend_resume(
                suspend_state=system_power_state_controller_interface.IdleSuspend(),
                resume_mode=system_power_state_controller_interface.ButtonPressResume(),
            )

    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_suspend",
        autospec=True,
    )
    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_set_resume_mode",
        autospec=True,
    )
    async def test_suspend_resume(
        self,
        mock_set_resume_mode: mock.AsyncMock,
        mock_suspend: mock.AsyncMock,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.suspend_resume()"""
        self.mock_inspect.get_inspect_data.side_effect = [
            fuchsia_inspect.InspectDataCollection.from_list(
                _SAG_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _FSH_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _SAG_INSPECT_DATA_AFTER
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _FSH_INSPECT_DATA_AFTER
            ),
        ]
        await self.system_power_state_controller_using_starnix_obj.suspend_resume(
            suspend_state=system_power_state_controller_interface.IdleSuspend(),
            resume_mode=system_power_state_controller_interface.TimerResume(
                duration=3,
            ),
        )
        mock_set_resume_mode.assert_called_once()
        mock_suspend.assert_called_once()

    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_suspend",
        autospec=True,
    )
    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_set_resume_mode",
        autospec=True,
    )
    async def test_suspend_resume_verify_using_sag_inspect_fail(
        self,
        mock_set_resume_mode: mock.AsyncMock,
        mock_suspend: mock.AsyncMock,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.suspend_resume()"""
        self.mock_inspect.get_inspect_data.side_effect = [
            fuchsia_inspect.InspectDataCollection.from_list(
                _SAG_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _FSH_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _SAG_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _FSH_INSPECT_DATA_AFTER
            ),
        ]
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Based on SAG inspect data",
        ):
            await self.system_power_state_controller_using_starnix_obj.suspend_resume(
                suspend_state=system_power_state_controller_interface.IdleSuspend(),
                resume_mode=system_power_state_controller_interface.TimerResume(
                    duration=3,
                ),
            )
        mock_set_resume_mode.assert_called_once()
        mock_suspend.assert_called_once()

    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_suspend",
        autospec=True,
    )
    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_set_resume_mode",
        autospec=True,
    )
    async def test_suspend_resume_verify_using_fsh_inspect_fail(
        self,
        mock_set_resume_mode: mock.AsyncMock,
        mock_suspend: mock.AsyncMock,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix.suspend_resume()"""
        self.mock_inspect.get_inspect_data.side_effect = [
            fuchsia_inspect.InspectDataCollection.from_list(
                _SAG_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _FSH_INSPECT_DATA_BEFORE
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _SAG_INSPECT_DATA_AFTER
            ),
            fuchsia_inspect.InspectDataCollection.from_list(
                _FSH_INSPECT_DATA_BEFORE
            ),
        ]
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Based on FSH inspect data",
        ):
            await self.system_power_state_controller_using_starnix_obj.suspend_resume(
                suspend_state=system_power_state_controller_interface.IdleSuspend(),
                resume_mode=system_power_state_controller_interface.TimerResume(
                    duration=3,
                ),
            )
        mock_set_resume_mode.assert_called_once()
        mock_suspend.assert_called_once()

    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_perform_idle_suspend",
        autospec=True,
    )
    async def test_suspend_with_idle_suspend(
        self,
        mock_perform_idle_suspend: mock.AsyncMock,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._suspend()"""
        await self.system_power_state_controller_using_starnix_obj._suspend(
            suspend_state=system_power_state_controller_interface.IdleSuspend(),
        )

        mock_perform_idle_suspend.assert_called_once_with(mock.ANY)

    async def test_perform_idle_suspend(
        self,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._perform_idle_suspend()"""
        self.mock_starnix.run_console_shell_cmd.return_value = "success"
        await self.system_power_state_controller_using_starnix_obj._perform_idle_suspend()

        self.mock_starnix.run_console_shell_cmd.assert_called_once_with(
            cmd=system_power_state_controller_using_starnix._StarnixCmds.IDLE_SUSPEND,
        )

    async def test_perform_idle_suspend_failure(
        self,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._perform_idle_suspend() raising exception"""
        self.mock_starnix.run_console_shell_cmd.side_effect = (
            starnix_errors.StarnixError("Error")
        )
        with self.assertRaises(
            system_power_state_controller_interface.SystemPowerStateControllerError
        ):
            await self.system_power_state_controller_using_starnix_obj._perform_idle_suspend()

        self.mock_starnix.run_console_shell_cmd.assert_called_once_with(
            cmd=system_power_state_controller_using_starnix._StarnixCmds.IDLE_SUSPEND,
        )

    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_wait_for_timer_end",
        autospec=True,
    )
    @mock.patch.object(
        system_power_state_controller_using_starnix.SystemPowerStateControllerUsingStarnix,
        "_wait_for_timer_start",
        autospec=True,
    )
    async def test_set_resume_mode_with_timer_resume(
        self,
        mock_wait_for_timer_start: mock.Mock,
        mock_wait_for_timer_end: mock.Mock,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._set_resume_mode() to
        perform timer based resume."""
        resume_mode: system_power_state_controller_interface.TimerResume = (
            system_power_state_controller_interface.TimerResume(
                duration=3,
            )
        )

        async with self.system_power_state_controller_using_starnix_obj._set_resume_mode(
            resume_mode=resume_mode,
        ):
            pass

        mock_wait_for_timer_start.assert_called_once_with(
            mock.ANY,
            proc=mock.ANY,
        )

        mock_wait_for_timer_end.assert_called_once_with(
            mock.ANY,
            proc=mock.ANY,
        )

    async def test_set_timer(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._set_timer() success case."""
        self.system_power_state_controller_using_starnix_obj._set_timer(
            duration=3
        )

    async def test_set_timer_failure(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._set_timer() raising
        an exception."""
        self.mock_ffx.popen.side_effect = RuntimeError("Error")

        with self.assertRaises(
            system_power_state_controller_interface.SystemPowerStateControllerError
        ):
            self.system_power_state_controller_using_starnix_obj._set_timer(
                duration=3
            )

    async def test_wait_for_timer_start(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._wait_for_timer_start()
        success case."""
        mock_subprocess_popen = mock.MagicMock(spec=subprocess.Popen)
        mock_subprocess_popen.stdout = mock.MagicMock(spec=io.TextIOWrapper)
        mock_subprocess_popen.stdout.readline.side_effect = _HRTIMER_CTL_OUTPUT

        self.system_power_state_controller_using_starnix_obj._wait_for_timer_start(
            proc=mock_subprocess_popen,
        )

    async def test_wait_for_timer_start_exception_1(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._wait_for_timer_start()
        raising SystemPowerStateControllerError exception because of not able to
        read command output."""
        mock_subprocess_popen = mock.MagicMock(spec=subprocess.Popen)
        mock_subprocess_popen.stdout = mock.MagicMock(spec=int)

        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Failed to read hrtimer-ctl output",
        ):
            self.system_power_state_controller_using_starnix_obj._wait_for_timer_start(
                proc=mock_subprocess_popen,
            )

    async def test_wait_for_timer_start_exception_2(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._wait_for_timer_start()
        raising SystemPowerStateControllerError exception because of timer
        start message was not found."""
        mock_subprocess_popen = mock.MagicMock(spec=subprocess.Popen)
        mock_subprocess_popen.stdout = mock.MagicMock(spec=io.TextIOWrapper)
        mock_subprocess_popen.stdout.readline.side_effect = (
            _HRTIMER_CTL_OUTPUT_NO_TIMER_START
        )

        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Timer has not been started on",
        ):
            self.system_power_state_controller_using_starnix_obj._wait_for_timer_start(
                proc=mock_subprocess_popen,
            )

    async def test_wait_for_timer_end(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._wait_for_timer_end()
        success case."""
        mock_subprocess_popen = mock.MagicMock(spec=subprocess.Popen)
        mock_subprocess_popen.communicate.return_value = (
            "\n".join(_HRTIMER_CTL_OUTPUT),
            None,
        )
        mock_subprocess_popen.returncode = 0

        self.system_power_state_controller_using_starnix_obj._wait_for_timer_end(
            proc=mock_subprocess_popen,
        )

    async def test_wait_for_timer_end_exception_1(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._wait_for_timer_end()
        raising SystemPowerStateControllerError exception because of timer
        end message was not found."""
        mock_subprocess_popen = mock.MagicMock(spec=subprocess.Popen)
        mock_subprocess_popen.communicate.return_value = (
            "\n".join(_HRTIMER_CTL_OUTPUT_NO_TIMER_END),
            None,
        )
        mock_subprocess_popen.returncode = 0

        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "hrtimer-ctl completed without ending the timer",
        ):
            self.system_power_state_controller_using_starnix_obj._wait_for_timer_end(
                proc=mock_subprocess_popen,
            )

    async def test_wait_for_timer_end_exception_2(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._wait_for_timer_end()
        raising SystemPowerStateControllerError exception because of failed to
        read command output."""
        mock_subprocess_popen = mock.MagicMock(spec=subprocess.Popen)
        mock_subprocess_popen.communicate.return_value = (
            None,
            "Mock Error",
        )
        mock_subprocess_popen.returncode = 1

        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "hrtimer-ctl returned a failure while waiting for the timer to "
            "end. returncode=1.*?error='Mock Error'",
        ):
            self.system_power_state_controller_using_starnix_obj._wait_for_timer_end(
                proc=mock_subprocess_popen,
            )

    async def test_get_suspend_stats_from_sag_inspect_data_fail(
        self,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._get_suspend_stats_from_sag_inspect_data()
        raising exception as it fails to read SAG inspect data."""
        self.mock_inspect.get_inspect_data.side_effect = errors.InspectError(
            "Inspect operation failed"
        )
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Failed to read SAG inspect data",
        ):
            await self.system_power_state_controller_using_starnix_obj._get_suspend_stats_from_sag_inspect_data()

    async def test_get_suspend_events_from_fsh_inspect_data_fail(
        self,
    ) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._get_suspend_events_from_fsh_inspect_data()
        raising exception as it fails to read FSH inspect data."""
        self.mock_inspect.get_inspect_data.side_effect = errors.InspectError(
            "Inspect operation failed"
        )
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Failed to read FSH inspect data",
        ):
            await self.system_power_state_controller_using_starnix_obj._get_suspend_events_from_fsh_inspect_data()

    async def test_validate_using_fsh_inspect_data_fail_1(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._validate_using_fsh_inspect_data()
        failure condition"""
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Based on FSH inspect data,",
        ):
            self.system_power_state_controller_using_starnix_obj._validate_using_fsh_inspect_data(
                suspend_state=system_power_state_controller_interface.IdleSuspend(),
                resume_mode=system_power_state_controller_interface.TimerResume(
                    duration=3
                ),
                suspend_resume_events_before={},
                suspend_resume_events_after={},
            )

    async def test_validate_using_fsh_inspect_data_fail_2(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._validate_using_fsh_inspect_data()
        failure condition"""
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Based on FSH inspect data,",
        ):
            self.system_power_state_controller_using_starnix_obj._validate_using_fsh_inspect_data(
                suspend_state=system_power_state_controller_interface.IdleSuspend(),
                resume_mode=system_power_state_controller_interface.TimerResume(
                    duration=3
                ),
                suspend_resume_events_before={},
                suspend_resume_events_after=SUSPEND_RESUME_EVENTS_AFTER_FAIL_2,
            )

    async def test_validate_using_fsh_inspect_data_fail_3(self) -> None:
        """Test case for SystemPowerStateControllerUsingStarnix._validate_using_fsh_inspect_data()
        failure condition"""
        with self.assertRaisesRegex(
            system_power_state_controller_interface.SystemPowerStateControllerError,
            "Expected it to not take more than",
        ):
            self.system_power_state_controller_using_starnix_obj._validate_using_fsh_inspect_data(
                suspend_state=system_power_state_controller_interface.IdleSuspend(),
                resume_mode=system_power_state_controller_interface.TimerResume(
                    duration=3
                ),
                suspend_resume_events_before={},
                suspend_resume_events_after=SUSPEND_RESUME_EVENTS_AFTER_FAIL_3,
            )


if __name__ == "__main__":
    unittest.main()
