#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""FFX host tool E2E stress tests."""

import logging

import ffxtestcase
from mobly import test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


# These tests just hammer the ffx connection to see if it falls over.
class FfxStressTest(ffxtestcase.FfxTestCase):
    """FFX host tool E2E stress test"""

    def test_stress(self) -> None:
        """Run multiple `ffx speedtest` instances to stress the ffx connection."""
        instances = []
        for i in range(20):
            cmd = [
                "speedtest",
                "-r",
                "5",
                "socket",
                "-L",
                "10",
                "-b",
                "1000",
            ]
            if i % 2 == 0:
                cmd += ["-R"]
            instances.append(self.spawn_ffx(cmd))

        for instance in instances:
            instance.wait()


if __name__ == "__main__":
    test_runner.main()
