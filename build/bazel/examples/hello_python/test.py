# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for lib.py"""

import unittest

from lib import Fib


class TestLib(unittest.TestCase):
    def test_Fib0(self):
        self.assertEqual(Fib(0), 0)

    def test_Fib1(self):
        self.assertEqual(Fib(1), 1)

    def test_Fib2(self):
        self.assertEqual(Fib(2), 1)

    def test_Fib3(self):
        self.assertEqual(Fib(3), 2)

    def test_Fib4(self):
        self.assertEqual(Fib(4), 3)

    def test_Fib5(self):
        self.assertEqual(Fib(5), 5)

    def test_Fib6(self):
        self.assertEqual(Fib(6), 8)


if __name__ == "__main__":
    unittest.main()
