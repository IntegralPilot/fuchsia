# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from multidict import multidict


class MultidictTest(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEquals(multidict([]), {})
        self.assertEquals(
            multidict([], value_container=set),
            {},
        )

    def test_simple(self) -> None:
        self.assertEquals(multidict([(1, 2)]), {1: [2]})
        self.assertEquals(
            multidict([(1, 2), (3, 4), (1, "a")]), {1: [2, "a"], 3: [4]}
        )
        self.assertEquals(
            multidict([(1, 2), (3, 4), (1, "a")], value_container=set),
            {1: {2, "a"}, 3: {4}},
        )


if __name__ == "__main__":
    unittest.main()
