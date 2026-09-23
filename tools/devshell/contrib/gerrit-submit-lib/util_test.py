#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import util


class TestUtil(unittest.TestCase):
    def test_exponential_backoff(self) -> None:
        clock = util.FakeClock()
        backoff = util.ExponentialBackoff(
            clock, min_poll_seconds=1.0, max_poll_seconds=10.0, backoff=2.0
        )
        backoff.wait()
        backoff.wait()
        backoff.wait()
        backoff.wait()
        backoff.wait()
        backoff.wait()
        self.assertEqual(clock.pauses, [1.0, 2.0, 4.0, 8.0, 10.0, 10.0])


if __name__ == "__main__":
    unittest.main()
