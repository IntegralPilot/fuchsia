# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import time

from base import Benchmark


class SampleBenchmark2(Benchmark):
    """A second sample benchmark that sleeps for a short time."""

    def __init__(self) -> None:
        super().__init__(
            name="sample2",
            description="A second sample benchmark that sleeps.",
        )

    def run(self) -> None:
        """Runs the benchmark."""
        time.sleep(0.1)
