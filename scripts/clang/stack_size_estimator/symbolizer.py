# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Symbolizer module for resolving addresses to source locations."""

import os
import subprocess
import sys
from typing import Any


class Symbolizer:
    """Wrapper around llvm-symbolizer for resolving addresses."""

    def __init__(self, bin_path: str, obj_file: str) -> None:
        self.bin_path = bin_path
        self.obj_file = obj_file
        self.process: subprocess.Popen[str] | None = None

    def __enter__(self) -> "Symbolizer":
        self._start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.close()

    def _start(self) -> None:
        if self.process:
            return

        if not os.path.exists(self.bin_path):
            raise FileNotFoundError(f"Symbolizer not found at {self.bin_path}")

        if not os.path.exists(self.obj_file):
            raise FileNotFoundError(f"Object file not found at {self.obj_file}")

        # Start llvm-symbolizer in interactive mode
        # --obj specifies the binary once, so we just feed addresses
        cmd = [
            self.bin_path,
            f"--obj={self.obj_file}",
            "--output-style=GNU",
            "--functions=none",
        ]
        # --functions=none because we just want file:line,
        # we have names from JSON.
        # --output-style=GNU gives "File:Line" instead of multiple lines.
        # Let's stick to default which is usually
        # Function
        # File:Line
        # Empty line

        try:
            self.process = subprocess.Popen(
                cmd,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=sys.stderr,
                text=True,
                bufsize=1,
            )
        except OSError as e:
            print(f"Failed to start symbolizer: {e}", file=sys.stderr)
            raise

    def symbolize(self, addresses: list[int]) -> dict[int, str]:
        """Symbolizes a list of addresses to file:line strings."""
        if not addresses:
            return {}

        self._start()
        if not self.process:
            return {}

        result = {}
        try:
            for addr in addresses:
                if self.process.stdin:
                    self.process.stdin.write(f"{hex(addr)}\n")
            if self.process.stdin:
                self.process.stdin.flush()

            for addr in addresses:
                # Read response.
                # With --output-style=GNU and --functions=none,
                # it should be one line per query.
                if self.process.stdout:
                    line = self.process.stdout.readline().strip()
                    result[addr] = line

        except (OSError, ValueError) as e:
            print(f"Error during symbolization: {e}", file=sys.stderr)

        return result

    def close(self) -> None:
        if self.process:
            self.process.terminate()
            self.process.wait()
            self.process = None
