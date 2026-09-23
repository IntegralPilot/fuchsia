# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for util."""

import unittest

import util


class TestUtil(unittest.TestCase):
    """Tests for util."""

    def test_sanitize_filename(self) -> None:
        """Test that sanitize_filename replaces invalid characters."""
        self.assertEqual(util.sanitize_filename("hello world"), "hello-world")
        self.assertEqual(util.sanitize_filename("hello/world"), "hello-world")
        self.assertEqual(util.sanitize_filename("hello..world"), "hello..world")
        self.assertEqual(util.sanitize_filename("hello-world"), "hello-world")
        self.assertEqual(util.sanitize_filename("hello_world"), "hello_world")
        self.assertEqual(util.sanitize_filename("Hello World"), "hello-world")
        self.assertEqual(
            util.sanitize_filename("hello!@#$%^&*()world"), "hello-world"
        )


if __name__ == "__main__":
    unittest.main()
