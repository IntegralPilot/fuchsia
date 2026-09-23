#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from mobly import asserts, signals

MSG_EXPECTED_EXCEPTION = "This is an expected exception."


class ActsAssertsTest(unittest.TestCase):
    """Verifies that asserts.xxx functions raise the correct test signals."""

    def test_assert_false(self):
        asserts.assert_false(False, MSG_EXPECTED_EXCEPTION)
        with self.assertRaisesRegexp(
            signals.TestFailure, MSG_EXPECTED_EXCEPTION
        ):
            asserts.assert_false(True, MSG_EXPECTED_EXCEPTION)


if __name__ == "__main__":
    unittest.main()
