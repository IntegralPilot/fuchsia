# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Merge size reports."""

import argparse
import json
import os
from typing import Any


def parse_args() -> argparse.Namespace:
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser(
        description="Merge size report JSON files."
    )
    parser.add_argument(
        "--size-budgets",
        help="Paths to size budgets, separated by comma",
    )
    parser.add_argument(
        "--size-reports",
        help="Paths to size-reports, separated by comma",
    )
    parser.add_argument(
        "--verbose-outputs",
        help="Paths to verbose outputs, separated by comma",
    )
    parser.add_argument(
        "--input-manifest",
        help="Path to JSON file listing input size report paths (from a GN metadata walk).",
    )
    parser.add_argument(
        "--merged-size-budgets",
        help="Path to output merged size budgets JSON file.",
    )
    parser.add_argument(
        "--merged-size-reports",
        help="Path to output merged size reports JSON file.",
    )
    parser.add_argument(
        "--merged-verbose-outputs",
        help="Path to output merged verbose outputs JSON file.",
    )
    parser.add_argument(
        "--depfile",
        help="Path to output depfile.",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    inputs_read: list[str] = []

    if args.merged_size_budgets:
        size_budget_table: dict[str, Any] = {"package_set_budgets": []}
        if args.size_budgets:
            for size_budget in args.size_budgets.split(","):
                inputs_read.append(size_budget)
                with open(size_budget, "r") as f:
                    budgets = json.load(f)
                    size_budget_table["package_set_budgets"].extend(
                        budgets.get("package_set_budgets", [])
                    )
        out_dir = os.path.dirname(args.merged_size_budgets)
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
        with open(args.merged_size_budgets, "w") as f:
            json.dump(size_budget_table, f, indent=4)

    if args.merged_verbose_outputs:
        verbose_output_table: dict[str, Any] = {}
        if args.verbose_outputs:
            for verbose_output in args.verbose_outputs.split(","):
                inputs_read.append(verbose_output)
                with open(verbose_output, "r") as f:
                    verbose_output_table.update(json.load(f))
        out_dir = os.path.dirname(args.merged_verbose_outputs)
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
        with open(args.merged_verbose_outputs, "w") as f:
            json.dump(verbose_output_table, f, indent=4)

    if args.merged_size_reports:
        size_report_table: dict[str, Any] = {}
        if args.size_reports:
            for size_report in args.size_reports.split(","):
                inputs_read.append(size_report)
                with open(size_report, "r") as f:
                    size_report_table.update(json.load(f))

        if args.input_manifest:
            inputs_read.append(args.input_manifest)
            with open(args.input_manifest, "r") as f:
                manifest_reports = json.load(f)
            for size_report in manifest_reports:
                inputs_read.append(size_report)
                with open(size_report, "r") as f:
                    size_report_table.update(json.load(f))

        out_dir = os.path.dirname(args.merged_size_reports)
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
        with open(args.merged_size_reports, "w") as f:
            json.dump(size_report_table, f, indent=4)

    if args.depfile:
        from depfile import DepFile

        depfile_dir = os.path.dirname(args.depfile)
        if depfile_dir:
            os.makedirs(depfile_dir, exist_ok=True)
        dep_file = DepFile.from_deps(args.merged_size_reports, inputs_read)
        with open(args.depfile, "w") as f:
            dep_file.write_to(f)


if __name__ == "__main__":
    main()
