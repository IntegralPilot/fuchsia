#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class TestPythonFormat(unittest.TestCase):
    def __init__(
        self,
        python_bin: Path,
        autoflake: Path,
        isort: Path,
        black: Path,
        script_path: Path,
        methodName: str = "runTest",
    ):
        super().__init__(methodName)
        self.python_bin = python_bin
        self.autoflake = autoflake
        self.isort = isort
        self.black = black
        self.script_path = script_path

    def test_pipeline(self) -> None:
        input_code = """import sys
import json
import os
from very_long_module_name_that_definitely_forces_isort_to_wrap_this_line import member_c, member_a, member_b

def foo():
    print(sys.argv)
    print(json.dumps({}))
    print(member_a, member_b, member_c)
    return 1+2
"""

        expected_output = """import json
import sys

from very_long_module_name_that_definitely_forces_isort_to_wrap_this_line import (
    member_a,
    member_b,
    member_c,
)


def foo():
    print(sys.argv)
    print(json.dumps({}))
    print(member_a, member_b, member_c)
    return 1 + 2
"""

        pyproject_content = """[tool.isort]
profile = "black"
line_length = 80
"""
        pyproject_file = Path("pyproject.toml").resolve()
        pyproject_file.write_text(pyproject_content)

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".py", delete=False, dir="."
        ) as f:
            f.write(input_code)
            temp_filepath = f.name

        try:
            cmd: list[str | Path] = [
                self.python_bin,
                self.script_path,
                "--python",
                self.python_bin,
                "--autoflake",
                self.autoflake,
                "--isort",
                self.isort,
                "--black",
                self.black,
                "--pyproject-toml",
                pyproject_file,
                temp_filepath,
            ]

            res = subprocess.run(cmd, capture_output=True, text=True)
            self.assertEqual(
                res.returncode, 0, f"Script failed with stderr:\n{res.stderr}"
            )

            self.assertEqual(res.stdout.strip(), expected_output.strip())

        finally:
            Path(temp_filepath).unlink(missing_ok=True)
            pyproject_file.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description="Run python format test.")
    parser.add_argument(
        "--python-bin", required=True, help="Path to python bin"
    )
    parser.add_argument(
        "--autoflake", required=True, help="Path to autoflake script"
    )
    parser.add_argument("--isort", required=True, help="Path to isort script")
    parser.add_argument("--black", required=True, help="Path to black binary")
    parser.add_argument(
        "--python-format-script",
        required=True,
        help="Path to python_format.py",
    )

    args = parser.parse_args()

    # Create custom suite to pass arguments to test instance
    suite = unittest.TestSuite()
    suite.addTest(
        TestPythonFormat(
            python_bin=Path(args.python_bin),
            autoflake=Path(args.autoflake),
            isort=Path(args.isort),
            black=Path(args.black),
            script_path=Path(args.python_format_script),
            methodName="test_pipeline",
        )
    )

    runner = unittest.TextTestRunner(verbosity=2)
    result = runner.run(suite)
    sys.exit(not result.wasSuccessful())


if __name__ == "__main__":
    main()
