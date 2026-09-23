# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import scipy.stats
import stats


class StatsTest(unittest.TestCase):
    def test_t_ppf_function(self):
        # Test our implementation of t_ppf() (the quantile function for
        # Student's t-distribution) against SciPy's implementation.

        # Test a few different values of p (cumulative probability).
        for p in [0.01, 0.1, 0.5, 0.95, 0.99]:
            # Test a few different values of df (degrees of freedom).
            for df in [0.5, 100, 1000] + list(range(1, 10)):
                with self.subTest(f"test p={p} df={df}"):
                    self.assertAlmostEqual(
                        stats.t_ppf(p, df),
                        scipy.stats.t.ppf(p, df),
                    )


if __name__ == "__main__":
    unittest.main()
