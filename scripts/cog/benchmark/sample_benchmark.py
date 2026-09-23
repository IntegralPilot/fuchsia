# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from base import Benchmark


class SampleBenchmark(Benchmark):
    """A sample benchmark that creates a directory and writes a file."""

    def __init__(self) -> None:
        super().__init__(
            name="sample",
            description="Creates a directory and writes a file.",
            compare=["sample2", "other-sample"],
        )

    def run(self) -> None:
        """Runs the benchmark."""
        assert self.temp_dir, "temp_dir not set"
        dir_path = self.temp_dir / "sample_dir"
        dir_path.mkdir()
        file_path = dir_path / "sample_file.txt"
        file_path.write_text("hello world")


class OtherSampleBenchmark(Benchmark):
    """A sample benchmark that creates a directory and writes a file."""

    def __init__(self) -> None:
        super().__init__(
            name="other-sample",
            description="Creates a directory and writes a file.",
        )

    def run(self) -> None:
        """Runs the benchmark."""
        assert False, "this benchmark always fails"
