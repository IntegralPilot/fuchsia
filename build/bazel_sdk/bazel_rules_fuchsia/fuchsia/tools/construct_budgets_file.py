# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generates a budgets configuration file for size checker tool.

This tool constructs a size budgets file from a list of packages.

Usage example:
  construct_budgets_file.py \
    --packages package_manifest1.json,package_manifest2.json \
    --name MyBudget \
    --budget 100 \
    --creep-budget 10 \
    --output out/default/size_budgets.json


The budget configuration is a JSON file used by `ffx assembly size-check`:
A budget has the following fields:
    name
    budget_bytes
    creep_budget_bytes
    packages: list of paths to package manifests
"""
import argparse
import json
import sys


def main():
    parser = argparse.ArgumentParser(
        description="Constructs a size budgets file"
    )
    parser.add_argument("--packages", type=str, required=True)
    parser.add_argument("--name", type=str, required=True)
    parser.add_argument("--budget", type=int, required=True)
    parser.add_argument("--creep-budget", type=int, required=True)
    parser.add_argument("--output", type=argparse.FileType("w"), required=True)
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()

    package_set_budgets = [
        {
            "name": args.name,
            "budget_bytes": args.budget,
            "creep_budget_bytes": args.creep_budget,
            "packages": args.packages.split(","),
        }
    ]

    output_contents = dict(
        package_set_budgets=package_set_budgets,
    )

    # Ensure the outputfile is closed early.
    with args.output as output:
        json.dump(output_contents, output, indent=2)


if __name__ == "__main__":
    sys.exit(main())
