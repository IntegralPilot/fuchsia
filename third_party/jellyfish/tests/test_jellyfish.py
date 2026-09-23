#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from jellyfish import _jellyfish as jellyfish


class JellyfishTest(unittest.TestCase):
    def test_jaro_winkler(self):
        # Ensure that we successfully imported the library and can call a
        # function.
        self.assertAlmostEqual(
            100 * jellyfish.jaro_winkler_similarity("jellyfish", "jellyfishes"),
            96.363,
            places=2,
        )


if __name__ == "__main__":
    unittest.main()
