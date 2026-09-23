#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import lib_for_test


class FooTest(unittest.TestCase):
    def test_foo(self):
        self.assertEqual(lib_for_test.foo(), 42)


if __name__ == "__main__":
    unittest.main()
