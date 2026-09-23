# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# An example test binary that contains unittest.TestCase subclasses.
# Used to verify the `--list_host_python_unittests` flag for host_py_test()
# binaries that depend on this script.

import unittest


class TestWithUnittests(unittest.TestCase):
    def test_something(self) -> None:
        self.assertEqual(1, 1)

    def test_something_else(self) -> None:
        self.assertEqual(2, 2)


if __name__ == "__main__":
    unittest.main()
