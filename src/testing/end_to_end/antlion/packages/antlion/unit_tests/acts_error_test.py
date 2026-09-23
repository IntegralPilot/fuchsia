#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import unittest

from antlion import error


class ActsErrorTest(unittest.TestCase):
    def test_assert_key_pulled_from_acts_error_code(self):
        e = error.ActsError()
        self.assertEqual(e.error_code, 100)

    def test_assert_description_pulled_from_docstring(self):
        e = error.ActsError()
        self.assertEqual(e.error_doc, "Base Acts Error")

    def test_error_without_args(self):
        e = error.ActsError()
        self.assertEqual(e.details, "")

    def test_error_with_args(self):
        args = ("hello",)
        e = error.ActsError(*args)
        self.assertEqual(e.details, "hello")

    def test_error_with_kwargs(self):
        e = error.ActsError(key="value")
        self.assertIn(("key", "value"), e.extras.items())


if __name__ == "__main__":
    unittest.main()
