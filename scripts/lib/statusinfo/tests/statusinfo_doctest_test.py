# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


import doctest
import unittest

import statusinfo.statusinfo


class TestStatusInfo(unittest.TestCase):
    def test_default_width_non_tty(self) -> None:
        """Calling duration_progress and status_progress without width should not crash when os.get_terminal_size fails."""
        import datetime
        import unittest.mock as mock

        with mock.patch(
            "os.get_terminal_size",
            side_effect=OSError(25, "Inappropriate ioctl for device"),
        ):
            dur_line = statusinfo.statusinfo.duration_progress(
                "Test", datetime.timedelta(seconds=5)
            )
            self.assertIn("Test", dur_line)

            prog_line = statusinfo.statusinfo.status_progress("Progress", 0.5)
            self.assertIn("Progress", prog_line)


def load_tests(
    _loader: unittest.TestLoader,
    tests: unittest.TestSuite,
    _ignore: unittest.TestLoader,
) -> unittest.TestSuite:
    tests.addTests(doctest.DocTestSuite(statusinfo.statusinfo))
    return tests
