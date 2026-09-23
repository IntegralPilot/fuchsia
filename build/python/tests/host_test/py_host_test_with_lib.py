#!/usr/bin/env fuchsia-vendored-python

# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import lib


class PyHostTestWithLibTests(unittest.TestCase):
    def test_truthy(self) -> None:
        self.assertEqual(lib.truthy(), True)

    def test_falsy(self) -> None:
        self.assertEqual(lib.falsy(), False)


if __name__ == "__main__":
    unittest.main()
