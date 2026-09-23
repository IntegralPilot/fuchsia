# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for control_flows.py"""

import unittest
from datetime import datetime, timedelta, timezone
from unittest import mock

from honeydew import errors
from honeydew.utils.control_flows import (
    RetryAbortingError,
    repeat_until_deadline,
    retry,
    retry_until_deadline,
)
from honeydew.utils.deadline import Deadline
from mobly import signals
from parameterized import param, parameterized


class _RetryAbortingErrorSubClass(RetryAbortingError):
    pass


class RetryTest(unittest.TestCase):
    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_retry_succeeds_on_first_try(
        self, mock_sleep: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock(return_value=1)
        await retry(
            task=mock_task,
            max_tries=3,
            retry_delay=timedelta(seconds=1),
        )

        mock_task.assert_called_once()
        mock_sleep.assert_not_called()

    @parameterized.expand(
        [
            param(expected_exception=RetryAbortingError),
            param(expected_exception=_RetryAbortingErrorSubClass),
            param(expected_exception=SyntaxError),
            param(expected_exception=signals.TestError),
            param(expected_exception=signals.TestFailure),
            param(expected_exception=signals.TestAbortClass),
            param(expected_exception=signals.TestAbortAll),
            param(expected_exception=errors.FatalDeviceError),
        ]
    )
    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_retry_does_not_retry_some_errors(
        self,
        mock_sleep_for_duration: mock.Mock,
        expected_exception: type[Exception],
    ) -> None:
        mock_task = mock.AsyncMock(side_effect=expected_exception("expected"))
        with self.assertRaisesRegex(
            expected_exception=expected_exception,
            expected_regex="expected",
        ):
            await retry(
                task=mock_task,
                max_tries=3,
                retry_delay=timedelta(seconds=1),
            )

        mock_task.assert_called_once()
        mock_sleep_for_duration.assert_not_called()

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_retry_succeeds_on_retry(
        self, mock_sleep_for_duration: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock(
            side_effect=2 * [RuntimeError("Triggers a retry")] + [True]
        )
        await retry(
            task=mock_task,
            max_tries=3,
            retry_delay=timedelta(seconds=1),
        )

        self.assertEqual(mock_task.call_count, 3)
        self.assertEqual(mock_sleep_for_duration.call_count, 2)

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_never_succeeds(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now
        mock_task = mock.AsyncMock(side_effect=RuntimeError("Triggers a retry"))
        with self.assertRaisesRegex(
            expected_exception=RuntimeError,
            expected_regex="Triggers a retry",
        ):
            await retry(
                task=mock_task,
                max_tries=3,
                retry_delay=timedelta(seconds=1),
            )

        self.assertEqual(mock_task.call_count, 3)
        self.assertEqual(mock_sleep.call_count, 2)
        mock_sleep.assert_has_calls(
            [mock.call(timedelta(seconds=1)), mock.call(timedelta(seconds=1))]
        )

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_never_succeeds_with_backoff(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now
        mock_task = mock.AsyncMock(side_effect=RuntimeError("Triggers a retry"))
        with self.assertRaisesRegex(
            expected_exception=RuntimeError,
            expected_regex="Triggers a retry",
        ):
            await retry(
                task=mock_task,
                max_tries=5,
                retry_delay=timedelta(seconds=1),
                backoff=True,
            )

        self.assertEqual(mock_task.call_count, 5)
        self.assertEqual(mock_sleep.call_count, 4)
        mock_sleep.assert_has_calls(
            [
                mock.call(timedelta(seconds=duration))
                for duration in [1, 2, 4, 8]
            ]
        )

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_with_deadline_hits_backoff_cap(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now
        mock_task = mock.AsyncMock(side_effect=RuntimeError("Triggers a retry"))
        with self.assertRaisesRegex(
            expected_exception=RuntimeError,
            expected_regex="Triggers a retry",
        ):
            await retry(
                task=mock_task,
                max_tries=5,
                retry_delay=timedelta(seconds=25),
                backoff=True,
            )

        self.assertEqual(mock_task.call_count, 5)

        expected_sleep_durations = [25, 50, 100, 200]

        expected_sleep_calls = [
            mock.call(timedelta(seconds=float(duration)))
            for duration in expected_sleep_durations
        ]
        mock_sleep.assert_has_calls(expected_sleep_calls)

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_retry_until_deadline_succeeds_on_first_try(
        self, mock_sleep: mock.Mock
    ) -> None:
        deadline = Deadline.from_timeout(timedelta(seconds=100))
        mock_task = mock.AsyncMock(return_value=1)
        await retry_until_deadline(
            task=mock_task,
            deadline=deadline,
            retry_delay=timedelta(seconds=1),
        )

        mock_task.assert_called_once()
        mock_sleep.assert_not_called()

    @parameterized.expand(
        [
            param(expected_exception=RetryAbortingError),
            param(expected_exception=_RetryAbortingErrorSubClass),
            param(expected_exception=SyntaxError),
            param(expected_exception=signals.TestFailure),
        ]
    )
    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_retry_until_deadline_does_not_retry_some_errors(
        self,
        mock_sleep: mock.Mock,
        expected_exception: type[Exception],
    ) -> None:
        mock_task = mock.AsyncMock(side_effect=expected_exception("expected"))
        mock_sleep.side_effect = AssertionError("sleep should not be called")
        with self.assertRaisesRegex(
            expected_exception=expected_exception,
            expected_regex="expected",
        ):
            await retry_until_deadline(
                task=mock_task,
                deadline=Deadline.from_timeout(timedelta(seconds=10)),
                retry_delay=timedelta(seconds=1),
            )

        mock_task.assert_called_once()
        mock_sleep.assert_not_called()

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_retry_until_deadline_succeeds_on_retry(
        self, mock_sleep: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock(
            side_effect=2 * [RuntimeError("Triggers a retry")] + [True]
        )
        await retry_until_deadline(
            task=mock_task,
            deadline=Deadline.from_timeout(timedelta(seconds=10)),
            retry_delay=timedelta(seconds=1),
        )

        self.assertEqual(mock_task.call_count, 3)
        self.assertEqual(mock_sleep.call_count, 2)

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_with_sleep(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now

        mock_task = mock.AsyncMock(
            side_effect=[RuntimeError("Triggers a retry"), True]
        )
        deadline = Deadline.from_timeout(timedelta(seconds=100))

        expected_sleep_duration = 45
        await retry_until_deadline(
            task=mock_task,
            deadline=deadline,
            retry_delay=timedelta(seconds=expected_sleep_duration),
        )

        self.assertEqual(mock_task.call_count, 2)
        self.assertEqual(mock_sleep.call_count, 1)
        mock_sleep.assert_called_once_with(
            timedelta(seconds=expected_sleep_duration)
        )

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_with_backoff(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now

        mock_task = mock.AsyncMock(
            side_effect=4 * [RuntimeError("Triggers a retry")] + [True]
        )
        deadline = Deadline.from_timeout(timedelta(seconds=100))
        await retry_until_deadline(
            task=mock_task,
            deadline=deadline,
            retry_delay=timedelta(seconds=1),
            backoff=True,
        )

        self.assertEqual(mock_task.call_count, 5)
        self.assertEqual(mock_sleep.call_count, 4)

        expected_sleep_durations = [1, 2, 4, 8]
        expected_sleep_calls = [
            mock.call(timedelta(seconds=duration))
            for duration in expected_sleep_durations
        ]
        mock_sleep.assert_has_calls(expected_sleep_calls)

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_hits_backoff_cap(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now

        mock_task = mock.AsyncMock(
            side_effect=4 * [RuntimeError("Triggers a retry")] + [True]
        )
        deadline = Deadline.from_timeout(
            timedelta(seconds=25 + 50 + 100 + 200 + 1)
        )
        await retry_until_deadline(
            task=mock_task,
            deadline=deadline,
            retry_delay=timedelta(seconds=25),
            backoff=True,
        )

        self.assertEqual(mock_task.call_count, 5)

        expected_sleep_durations = [
            25,
            50,
            100,
            200,
        ]
        expected_sleep_calls = [
            mock.call(timedelta(seconds=float(duration)))
            for duration in expected_sleep_durations
        ]
        mock_sleep.assert_has_calls(expected_sleep_calls)

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_retry_until_deadline_never_succeeds(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock(side_effect=RuntimeError("Triggers a retry"))
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )
        deadline = Deadline.from_timeout(timedelta(seconds=10))

        # Script datetime.now() to jump past the deadline after the task triggers twice
        def sleep_func(unused_duration: timedelta) -> None:
            if mock_task.call_count == 2:
                mock_datetime.now.return_value += timedelta(seconds=20)

        mock_sleep.side_effect = sleep_func

        with self.assertRaisesRegex(
            expected_exception=RuntimeError,
            expected_regex="Triggers a retry",
        ):
            await retry_until_deadline(
                task=mock_task,
                deadline=deadline,
                retry_delay=timedelta(seconds=1),
            )

        self.assertEqual(mock_task.call_count, 3)
        self.assertEqual(mock_sleep.call_count, 2)


class RepeatTest(unittest.TestCase):
    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_repeat_until_deadline(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock()
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )
        deadline = Deadline.from_timeout(timedelta(seconds=10))

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now

        await repeat_until_deadline(
            task=mock_task,
            deadline=deadline,
            repeat_delay=timedelta(seconds=1),
        )

        self.assertEqual(mock_task.call_count, 10)
        self.assertEqual(mock_sleep.call_count, 9)

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    async def test_repeat_until_deadline_when_already_due(
        self, mock_sleep: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock()
        deadline = Deadline.from_timeout(timedelta(seconds=0))

        await repeat_until_deadline(
            task=mock_task,
            deadline=deadline,
            repeat_delay=timedelta(seconds=1),
        )

        mock_task.assert_not_called()
        mock_sleep.assert_not_called()

    @mock.patch("honeydew.utils.control_flows.sleep_for_duration")
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    async def test_repeat_until_deadline_raises_exception(
        self, mock_datetime: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        mock_task = mock.AsyncMock(
            side_effect=4 * [None] + [RuntimeError("Expected")]
        )
        mock_datetime.now.return_value = datetime(
            2024, 7, 10, tzinfo=timezone.utc
        )
        deadline = Deadline.from_timeout(timedelta(seconds=10))

        def advance_now(delta: timedelta) -> None:
            mock_datetime.now.return_value += delta

        mock_sleep.side_effect = advance_now

        with self.assertRaisesRegex(RuntimeError, "Expected"):
            await repeat_until_deadline(
                task=mock_task,
                deadline=deadline,
                repeat_delay=timedelta(seconds=1),
            )

        self.assertEqual(mock_task.call_count, 5)
        self.assertEqual(mock_sleep.call_count, 4)


if __name__ == "__main__":
    unittest.main()
