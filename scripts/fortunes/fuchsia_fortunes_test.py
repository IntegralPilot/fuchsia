#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import unittest

import fuchsia_fortunes


class MainTests(unittest.TestCase):
    """Really just tests that there are no syntax errors."""

    def test_random(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            fuchsia_fortunes.main([])

        # output is random, so there is nothing to compare


if __name__ == "__main__":
    unittest.main()
