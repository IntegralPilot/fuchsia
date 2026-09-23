# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.utils.power.py."""

import datetime
import unittest
from datetime import timedelta, timezone
from typing import Any
from unittest import mock

import fuchsia_inspect
from honeydew import errors
from honeydew.fuchsia_device import fuchsia_device
from honeydew.transports.ffx import types as ffx_types
from honeydew.utils import control_flows, power
from honeydew.utils.deadline import Deadline


class PowerTests(unittest.IsolatedAsyncioTestCase):
    """Unit tests for honeydew.utils.power."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_device = mock.MagicMock(spec=fuchsia_device.FuchsiaDevice)
        self.mock_device.device_name = "test-device"

    async def test_get_sag_suspend_stats_success(self) -> None:
        """Test case for get_sag_suspend_stats() success case."""
        payload: dict[str, Any] = {
            "root": {
                "suspend_stats": {
                    "success_count": 10,
                    "fail_count": 1,
                    "total_time_in_suspend_ns": 1000000,
                }
            }
        }
        inspect_data = fuchsia_inspect.InspectData(
            moniker="bootstrap/system-activity-governor",
            metadata=fuchsia_inspect.InspectMetadata(
                timestamp=fuchsia_inspect.Timestamp(12345)
            ),
            payload=payload,
            version=1,
        )
        self.mock_device.get_inspect_data.return_value = (
            fuchsia_inspect.InspectDataCollection(data=[inspect_data])
        )

        stats = await power.get_sag_suspend_stats(self.mock_device)

        self.assertEqual(stats.success_count, 10)
        self.assertEqual(stats.fail_count, 1)
        self.assertEqual(
            stats.total_time_in_suspend, timedelta(microseconds=1000)
        )

        self.mock_device.get_inspect_data.assert_called_once_with(
            selectors=["bootstrap/system-activity-governor:root"]
        )

    async def test_get_sag_suspend_stats_empty_data(self) -> None:
        """Test case for get_sag_suspend_stats() when no data is returned."""
        self.mock_device.get_inspect_data.return_value = (
            fuchsia_inspect.InspectDataCollection(data=[])
        )

        with self.assertRaisesRegex(errors.InspectError, "is empty"):
            await power.get_sag_suspend_stats(self.mock_device)

    async def test_get_sag_suspend_stats_missing_payload(self) -> None:
        """Test case for get_sag_suspend_stats() when payload is missing."""
        inspect_data = fuchsia_inspect.InspectData(
            moniker="bootstrap/system-activity-governor",
            metadata=fuchsia_inspect.InspectMetadata(
                timestamp=fuchsia_inspect.Timestamp(12345)
            ),
            payload=None,
            version=1,
        )
        self.mock_device.get_inspect_data.return_value = (
            fuchsia_inspect.InspectDataCollection(data=[inspect_data])
        )

        with self.assertRaisesRegex(
            errors.InspectError, "not have a valid payload"
        ):
            await power.get_sag_suspend_stats(self.mock_device)

    async def test_get_sag_suspend_stats_missing_field(self) -> None:
        """Test case for get_sag_suspend_stats() when expected field is missing."""
        payload: dict[str, Any] = {
            "root": {
                # missing suspend_stats
            }
        }
        inspect_data = fuchsia_inspect.InspectData(
            moniker="bootstrap/system-activity-governor",
            metadata=fuchsia_inspect.InspectMetadata(
                timestamp=fuchsia_inspect.Timestamp(12345)
            ),
            payload=payload,
            version=1,
        )
        self.mock_device.get_inspect_data.return_value = (
            fuchsia_inspect.InspectDataCollection(data=[inspect_data])
        )

        with self.assertRaisesRegex(
            errors.InspectError, "missing expected field"
        ):
            await power.get_sag_suspend_stats(self.mock_device)

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime.datetime)
    @mock.patch.object(control_flows, "sleep_until_deadline")
    @mock.patch.object(power, "get_sag_suspend_stats")
    async def test_suspend_resume_exception_during_suspend(
        self,
        mock_get_stats: mock.MagicMock,
        mock_sleep: mock.MagicMock,
        mock_datetime: mock.MagicMock,
    ) -> None:
        """Test case for suspend_resume() when suspend fails."""
        t0 = datetime.datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        mock_datetime.now.return_value = t0
        deadline = Deadline.from_timeout(timedelta(seconds=5))

        stats_before = power.SagSuspendStats(
            success_count=10,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=1),
        )
        mock_get_stats.return_value = stats_before

        self.mock_device.suspend.side_effect = RuntimeError("Suspend failed")

        with self.assertRaisesRegex(RuntimeError, "Suspend failed"):
            await power.suspend_resume(self.mock_device, deadline)

        self.mock_device.suspend.assert_called_once()
        mock_sleep.assert_not_called()
        self.mock_device.resume.assert_called_once()
        self.mock_device.ffx.run.assert_called_once_with(
            ["session", "drop-power-lease", "--allow-missing"],
            # TODO(b/492542002) "ffx session drop-power-lease" should support "--machine json"
            machine=ffx_types.MachineFormat.RAW,
        )

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime.datetime)
    @mock.patch.object(control_flows, "sleep_until_deadline")
    @mock.patch.object(power, "get_sag_suspend_stats")
    async def test_suspend_resume_success(
        self,
        mock_get_stats: mock.MagicMock,
        mock_sleep: mock.MagicMock,
        mock_datetime: mock.MagicMock,
    ) -> None:
        """Test case for suspend_resume() success."""
        t0 = datetime.datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        mock_datetime.now.return_value = t0
        deadline = Deadline.from_timeout(timedelta(minutes=5))

        stats_before = power.SagSuspendStats(
            success_count=10,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=1),
        )
        stats_after = power.SagSuspendStats(
            success_count=11,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=2),
        )
        mock_get_stats.side_effect = [stats_before, stats_after]

        await power.suspend_resume(self.mock_device, deadline)

        self.mock_device.ffx.run.assert_called_once_with(
            ["session", "drop-power-lease", "--allow-missing"],
            # TODO(b/492542002) "ffx session drop-power-lease" should support "--machine json"
            machine=ffx_types.MachineFormat.RAW,
        )
        self.mock_device.suspend.assert_called_once()
        mock_sleep.assert_called_once()
        self.mock_device.resume.assert_called_once()
        self.assertEqual(mock_get_stats.call_count, 2)

    @mock.patch("honeydew.utils.deadline.datetime")
    @mock.patch.object(control_flows, "sleep_until_deadline")
    @mock.patch.object(power, "get_sag_suspend_stats")
    async def test_suspend_resume_retry_success(
        self,
        mock_get_stats: mock.MagicMock,
        mock_sleep: mock.MagicMock,
        mock_datetime: mock.MagicMock,
    ) -> None:
        """Test case for suspend_resume() with a retry."""
        t0 = datetime.datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        mock_datetime.now.return_value = t0
        deadline = Deadline.from_timeout(timedelta(minutes=5))

        stats_before_1 = power.SagSuspendStats(
            success_count=10,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=1),
        )
        stats_after_1 = power.SagSuspendStats(
            success_count=10,
            fail_count=2,
            total_time_in_suspend=timedelta(seconds=1),
        )
        stats_before_2 = power.SagSuspendStats(
            success_count=10,
            fail_count=2,
            total_time_in_suspend=timedelta(seconds=1),
        )
        stats_after_2 = power.SagSuspendStats(
            success_count=11,
            fail_count=2,
            total_time_in_suspend=timedelta(seconds=2),
        )

        mock_get_stats.side_effect = [
            stats_before_1,
            stats_after_1,
            stats_before_2,
            stats_after_2,
        ]

        await power.suspend_resume(self.mock_device, deadline)

        self.mock_device.ffx.run.assert_called_once_with(
            ["session", "drop-power-lease", "--allow-missing"],
            # TODO(b/492542002) "ffx session drop-power-lease" should support "--machine json"
            machine=ffx_types.MachineFormat.RAW,
        )
        self.assertEqual(self.mock_device.suspend.call_count, 2)
        self.assertEqual(mock_sleep.call_count, 2)
        self.assertEqual(self.mock_device.resume.call_count, 2)
        self.assertEqual(mock_get_stats.call_count, 4)

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime.datetime)
    @mock.patch.object(control_flows, "sleep_until_deadline")
    @mock.patch.object(power, "get_sag_suspend_stats")
    async def test_suspend_resume_timeout(
        self,
        mock_get_stats: mock.MagicMock,
        mock_sleep: mock.MagicMock,
        mock_datetime: mock.MagicMock,
    ) -> None:
        """Test case for suspend_resume() timeout."""
        t0 = datetime.datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        t_due = t0 + timedelta(minutes=10)

        def mock_now(
            unused_tz: datetime.timezone | None = None,
        ) -> datetime.datetime:
            if mock_sleep.call_count > 0:
                return t_due
            return t0

        mock_datetime.now.side_effect = mock_now
        deadline = Deadline.from_timeout(timedelta(minutes=5))

        stats_before = power.SagSuspendStats(
            success_count=10,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=1),
        )
        stats_after = power.SagSuspendStats(
            success_count=10,
            fail_count=2,
            total_time_in_suspend=timedelta(seconds=1),
        )
        mock_get_stats.side_effect = [stats_before, stats_after]

        with self.assertRaisesRegex(
            power.DeviceDidNotSuspendError, "SAG did not suspend during idle."
        ):
            await power.suspend_resume(self.mock_device, deadline)

        self.mock_device.ffx.run.assert_called_once_with(
            ["session", "drop-power-lease", "--allow-missing"],
            # TODO(b/492542002) "ffx session drop-power-lease" should support "--machine json"
            machine=ffx_types.MachineFormat.RAW,
        )
        self.mock_device.suspend.assert_called_once()
        mock_sleep.assert_called_once()
        self.mock_device.resume.assert_called_once()
        self.assertEqual(mock_get_stats.call_count, 2)

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime.datetime)
    @mock.patch.object(control_flows, "sleep_until_deadline")
    @mock.patch.object(power, "get_sag_suspend_stats")
    async def test_suspend_resume_ffx_error_raises(
        self,
        unused_mock_get_stats: mock.MagicMock,
        unused_mock_sleep: mock.MagicMock,
        mock_datetime: mock.MagicMock,
    ) -> None:
        """Test case for suspend_resume() when ffx session drop-power-lease fails."""
        self.mock_device.ffx.run.side_effect = RuntimeError("ffx failed")
        t0 = datetime.datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        mock_datetime.now.return_value = t0
        deadline = Deadline.from_timeout(timedelta(minutes=5))

        with self.assertRaisesRegex(RuntimeError, "ffx failed"):
            await power.suspend_resume(self.mock_device, deadline)

        self.mock_device.ffx.run.assert_called_once_with(
            ["session", "drop-power-lease", "--allow-missing"],
            # TODO(b/492542002) "ffx session drop-power-lease" should support "--machine json"
            machine=ffx_types.MachineFormat.RAW,
        )

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime.datetime)
    @mock.patch.object(control_flows, "sleep_until_deadline")
    @mock.patch.object(power, "get_sag_suspend_stats")
    async def test_suspend_resume_no_deadline(
        self,
        mock_get_stats: mock.MagicMock,
        unused_mock_sleep: mock.MagicMock,
        mock_datetime: mock.MagicMock,
    ) -> None:
        """Test case for suspend_resume() without a deadline."""
        t0 = datetime.datetime(2025, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        mock_datetime.now.return_value = t0

        stats_before = power.SagSuspendStats(
            success_count=10,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=1),
        )
        stats_after = power.SagSuspendStats(
            success_count=11,
            fail_count=1,
            total_time_in_suspend=timedelta(seconds=2),
        )
        mock_get_stats.side_effect = [stats_before, stats_after]

        await power.suspend_resume(self.mock_device)

        self.mock_device.ffx.run.assert_called_once_with(
            ["session", "drop-power-lease", "--allow-missing"],
            # TODO(b/492542002) "ffx session drop-power-lease" should support "--machine json"
            machine=ffx_types.MachineFormat.RAW,
        )


if __name__ == "__main__":
    unittest.main()
