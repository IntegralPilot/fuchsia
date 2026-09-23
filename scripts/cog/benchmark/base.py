# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import abc
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import List, Optional


class Benchmark(abc.ABC):
    """Abstract base class for benchmarks."""

    def __init__(
        self,
        name: str,
        description: str,
        expected_to_pass: bool = True,
        compare: Optional[List[str]] = None,
    ):
        """Initializes the Benchmark.

        Args:
            name: The name of the benchmark.
            description: The description of the benchmark.
            expected_to_pass: Whether the benchmark is expected to pass.
            compare: A list of other benchmark names to compare against.
        """
        self.name = name
        self.description = description
        self.expected_to_pass = expected_to_pass
        self.compare = compare or []
        self.temp_dir: Optional[Path] = None

    def setup(self) -> None:
        """Sets up the benchmark. This method is not timed."""
        self.temp_dir = Path(tempfile.mkdtemp())

    def cleanup(self) -> None:
        """Cleans up after the benchmark. This method is not timed."""
        if self.temp_dir:
            shutil.rmtree(self.temp_dir)
            self.temp_dir = None

    def run_command(
        self, command: List[str], cwd: Optional[str] = None
    ) -> subprocess.CompletedProcess[str]:
        """Runs a command as a subprocess.

        Args:
            command: The command to run as a list of strings.
            cwd: The working directory for the command.

        Returns:
            The completed process.
        """
        return subprocess.run(
            command,
            cwd=cwd,
            check=True,
            capture_output=True,
            text=True,
        )

    @abc.abstractmethod
    def run(self) -> None:
        """Runs the benchmark."""
