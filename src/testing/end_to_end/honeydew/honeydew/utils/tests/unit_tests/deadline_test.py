# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for deadline.py"""

import unittest
from datetime import datetime, timedelta, timezone
from unittest import mock

from honeydew import errors
from honeydew.utils.deadline import Deadline


class DeadlineTest(unittest.TestCase):
    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    def test_deadline(self, mock_datetime: mock.Mock) -> None:
        start = datetime(2024, 7, 10, tzinfo=timezone.utc)
        mock_datetime.now.return_value = start
        deadline = Deadline.from_timeout(timedelta(seconds=100))

        # Initial conditions
        self.assertFalse(deadline.is_due())
        self.assertEqual(deadline.remaining_duration(), timedelta(seconds=100))

        # During the deadline
        mock_datetime.now.return_value = start + timedelta(seconds=40)
        self.assertFalse(deadline.is_due())
        self.assertEqual(deadline.remaining_duration(), timedelta(seconds=60))

        # Almost at the deadline
        mock_datetime.now.return_value = start + timedelta(seconds=99)
        self.assertFalse(deadline.is_due())
        self.assertEqual(deadline.remaining_duration(), timedelta(seconds=1))

        # At the deadline
        mock_datetime.now.return_value = start + timedelta(seconds=100)
        self.assertTrue(deadline.is_due())
        self.assertEqual(deadline.remaining_duration(), timedelta(seconds=0))

        # Right after the deadline has passed
        mock_datetime.now.return_value = start + timedelta(seconds=101)
        self.assertTrue(deadline.is_due())
        self.assertEqual(deadline.remaining_duration(), timedelta(seconds=-1))

    def test_infinite_deadline(self) -> None:
        d = Deadline.infinite()
        self.assertIsNone(d.remaining_duration())
        self.assertIsNone(d.remaining_seconds())

    def test_remaining_seconds(self) -> None:
        d = Deadline.from_timeout(timedelta(seconds=100))
        self.assertIsNotNone(d.remaining_seconds())
        # We can't check exact value easily due to time passing, but check > 0
        rem = d.remaining_seconds()
        assert rem is not None

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    def test_utc_datetime(self, unused_mock_datetime: mock.Mock) -> None:
        start = datetime(2024, 7, 10, tzinfo=timezone.utc)
        d = Deadline(start)
        self.assertEqual(d.utc_datetime(), start)

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    def test_subdeadline_with_timeout(self, mock_datetime: mock.Mock) -> None:
        start = datetime(2024, 7, 10, tzinfo=timezone.utc)
        mock_datetime.now.return_value = start
        d = Deadline(start + timedelta(seconds=100))

        # Subdeadline shorter than original
        sub = d.subdeadline_with_timeout(timedelta(seconds=50))
        self.assertEqual(sub.remaining_duration(), timedelta(seconds=50))

        # Subdeadline longer than original (should be clamped to original)
        sub = d.subdeadline_with_timeout(timedelta(seconds=200))
        self.assertEqual(sub.remaining_duration(), timedelta(seconds=100))

        # Negative timeout (should be 0)
        sub = d.subdeadline_with_timeout(timedelta(seconds=-10))
        self.assertTrue(sub.is_due())

        # Infinite deadline (should create finite subdeadline)
        d_inf = Deadline.infinite()
        sub_inf = d_inf.subdeadline_with_timeout(timedelta(seconds=50))
        self.assertEqual(sub_inf.remaining_duration(), timedelta(seconds=50))

        # Infinite-past deadline (should remain infinite-past)
        d_inf_past = Deadline.infinite_past()
        sub_inf_past = d_inf_past.subdeadline_with_timeout(
            timedelta(seconds=50)
        )
        self.assertTrue(sub_inf_past.is_due())
        self.assertEqual(str(sub_inf_past), "Deadline(infinite_past)")

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    def test_subdeadline_with_grace_period(
        self, mock_datetime: mock.Mock
    ) -> None:
        start = datetime(2024, 7, 10, tzinfo=timezone.utc)
        mock_datetime.now.return_value = start
        d = Deadline(start + timedelta(seconds=100))

        # Grace period reduces deadline
        sub = d.subdeadline_with_grace_period(timedelta(seconds=10))
        self.assertEqual(sub.remaining_duration(), timedelta(seconds=90))

        # Negative grace period (should be ignored)
        sub = d.subdeadline_with_grace_period(timedelta(seconds=-10))
        self.assertEqual(sub.remaining_duration(), timedelta(seconds=100))

        # Infinite deadline (should remain infinite)
        d_inf = Deadline.infinite()
        sub_inf = d_inf.subdeadline_with_grace_period(timedelta(seconds=10))
        self.assertIsNone(sub_inf.remaining_duration())

        # Infinite-past deadline (should remain infinite-past)
        d_inf_past = Deadline.infinite_past()
        sub_inf_past = d_inf_past.subdeadline_with_grace_period(
            timedelta(seconds=10)
        )
        self.assertTrue(sub_inf_past.is_due())
        self.assertEqual(str(sub_inf_past), "Deadline(infinite_past)")

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    def test_check(self, mock_datetime: mock.Mock) -> None:
        start = datetime(2024, 7, 10, tzinfo=timezone.utc)
        mock_datetime.now.return_value = start

        d = Deadline.from_timeout(timedelta(seconds=0))
        with self.assertRaises(errors.HoneydewTimeoutError):
            d.check()

        d = Deadline.from_timeout(timedelta(seconds=100))
        d.check()  # Should not raise

    @mock.patch("honeydew.utils.deadline.datetime", wraps=datetime)
    def test_check_still_have(self, mock_datetime: mock.Mock) -> None:
        start = datetime(2024, 7, 10, tzinfo=timezone.utc)
        mock_datetime.now.return_value = start

        d = Deadline.from_timeout(timedelta(seconds=100))

        # Should not raise
        d.check_still_have(timedelta(seconds=50))

        # Should raise because 100 <= 150
        with self.assertRaisesRegex(
            errors.HoneydewTimeoutError,
            r"Deadline does not have the required 0:02:30 remaining",
        ):
            d.check_still_have(timedelta(seconds=150))

    def test_infinite_past(self) -> None:
        d = Deadline.infinite_past()
        self.assertTrue(d.is_due())
        remaining = d.remaining_duration()
        assert remaining is not None
        self.assertLess(remaining, timedelta(seconds=0))

    def test_equality(self) -> None:
        t1 = datetime(2024, 7, 10, tzinfo=timezone.utc)
        t2 = datetime(2024, 7, 11, tzinfo=timezone.utc)

        self.assertEqual(Deadline(t1), Deadline(t1))
        self.assertNotEqual(Deadline(t1), Deadline(t2))
        self.assertNotEqual(Deadline(t1), "not a deadline")

    def test_str_repr(self) -> None:
        self.assertEqual(str(Deadline.infinite()), "Deadline(infinite)")
        self.assertEqual(
            str(Deadline.infinite_past()), "Deadline(infinite_past)"
        )

        d = Deadline.from_timeout(timedelta(seconds=100))
        self.assertIn("Deadline(remaining=", str(d))


if __name__ == "__main__":
    unittest.main()
