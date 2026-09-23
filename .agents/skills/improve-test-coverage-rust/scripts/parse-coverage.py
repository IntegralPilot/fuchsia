#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys


def find_fuchsia_root():
    # 1. Try environment variable
    if "FUCHSIA_DIR" in os.environ:
        return os.environ["FUCHSIA_DIR"]

    # 2. Try relative to script location
    try:
        script_dir = os.path.dirname(os.path.abspath(__file__))
        potential_root = os.path.abspath(
            os.path.join(script_dir, "..", "..", "..", "..")
        )
        if os.path.exists(os.path.join(potential_root, ".fx-root")):
            return potential_root
    except Exception:
        pass

    # 3. Try current working directory or parent directories
    cwd = os.getcwd()
    while cwd != os.path.dirname(cwd):
        if os.path.exists(os.path.join(cwd, ".fx-root")):
            return cwd
        cwd = os.path.dirname(cwd)

    # Fallback to current working directory
    return os.getcwd()


def parse_lcov(file_path, filter_prefix):
    files_coverage = {}
    current_file = None

    with open(file_path, "r") as f:
        for line in f:
            line = line.strip()
            if line.startswith("SF:"):
                source_file = line[3:]
                if source_file.startswith(filter_prefix):
                    current_file = source_file
                    files_coverage[current_file] = {
                        "lines": {},
                        "total_instrumented": 0,
                        "total_covered": 0,
                    }
                else:
                    current_file = None
            elif current_file and line.startswith("DA:"):
                parts = line[3:].split(",")
                if len(parts) == 2:
                    line_num = int(parts[0])
                    count = int(parts[1])
                    files_coverage[current_file]["lines"][line_num] = count
                    files_coverage[current_file]["total_instrumented"] += 1
                    if count > 0:
                        files_coverage[current_file]["total_covered"] += 1
            elif current_file and line == "end_of_record":
                current_file = None

    return files_coverage


def report_coverage(coverage_data, filter_prefix):
    print(
        f"{'File':<50} | {'Covered':<8} / {'Total':<8} | {'Percentage':<10} | {'Uncovered Lines'}"
    )
    print("-" * 100)

    grand_total_instrumented = 0
    grand_total_covered = 0

    sorted_files = sorted(coverage_data.keys())
    for file_path in sorted_files:
        data = coverage_data[file_path]
        total = data["total_instrumented"]
        covered = data["total_covered"]

        pct = (covered / total) * 100.0 if total > 0 else 0.0
        grand_total_instrumented += total
        grand_total_covered += covered

        uncovered_lines = [
            line for line, count in sorted(data["lines"].items()) if count == 0
        ]

        # Format uncovered ranges (e.g., "10-15, 20")
        ranges = []
        if uncovered_lines:
            start = uncovered_lines[0]
            prev = uncovered_lines[0]
            for val in uncovered_lines[1:]:
                if val == prev + 1:
                    prev = val
                else:
                    ranges.append(
                        str(start) if start == prev else f"{start}-{prev}"
                    )
                    start = val
                    prev = val
            ranges.append(str(start) if start == prev else f"{start}-{prev}")

        uncovered_str = ", ".join(ranges)
        short_name = os.path.relpath(file_path, filter_prefix)
        print(
            f"{short_name:<50} | {covered:<8} / {total:<8} | {pct:>9.2f}% | {uncovered_str}"
        )

    print("-" * 100)
    grand_pct = (
        (grand_total_covered / grand_total_instrumented) * 100.0
        if grand_total_instrumented > 0
        else 0.0
    )
    print(
        f"{'GRAND TOTAL':<50} | {grand_total_covered:<8} / {grand_total_instrumented:<8} | {grand_pct:>9.2f}%"
    )


if __name__ == "__main__":
    fuchsia_root = find_fuchsia_root()

    lcov_file = os.path.join(fuchsia_root, "lcov.info")
    prefix = fuchsia_root

    if len(sys.argv) > 1:
        lcov_file = sys.argv[1]
    if len(sys.argv) > 2:
        prefix = sys.argv[2]
        if not os.path.isabs(prefix):
            prefix = os.path.join(fuchsia_root, prefix)
        prefix = os.path.abspath(prefix)

    # Ensure prefix ends with a separator if it is a directory
    if os.path.isdir(prefix) and not prefix.endswith(os.sep):
        prefix += os.sep
    elif (
        not os.path.isdir(prefix)
        and not prefix.endswith(os.sep)
        and not os.path.isfile(prefix)
    ):
        prefix += os.sep

    if not os.path.exists(lcov_file):
        print(f"Error: {lcov_file} does not exist.")
        sys.exit(1)

    data = parse_lcov(lcov_file, prefix)
    report_coverage(data, prefix)
