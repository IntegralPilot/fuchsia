#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import time
import unittest

from mobly import logger


class ActsLoggerTest(unittest.TestCase):
    """Verifies code in antlion.logger module."""

    def test_epoch_to_log_line_timestamp(self):
        os.environ["TZ"] = "US/Pacific"
        time.tzset()
        actual_stamp = logger.epoch_to_log_line_timestamp(1469134262116)
        self.assertEqual("2016-07-21 13:51:02.116", actual_stamp)


if __name__ == "__main__":
    unittest.main()
