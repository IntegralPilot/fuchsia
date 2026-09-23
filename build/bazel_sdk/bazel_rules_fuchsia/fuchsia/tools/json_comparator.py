# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tool to compare two json file against golden file regardless of order."""

import argparse
import json
import sys


def parse_args():
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--generated",
        help="Path to generated file",
        required=True,
    )
    parser.add_argument(
        "--golden",
        help="Path to the golden file",
        required=True,
    )
    parser.add_argument(
        "--subset",
        help="Whether to check if golden is a subset of generated",
        action="store_true",
    )
    return parser.parse_args()


def sorting(item):
    if isinstance(item, dict):
        return sorted((key, sorting(values)) for key, values in item.items())
    if isinstance(item, list):
        return sorted(sorting(x) for x in item)
    return item


def is_subset(golden, generated):
    if isinstance(golden, dict):
        if not isinstance(generated, dict):
            print("Generated is expected to be a dict, but is not")
            return False

        for key, value in golden.items():
            if key not in generated:
                print("Generated file does not contain key: " + key)
                return False
            if not is_subset(value, generated[key]):
                print("Generated value does not match for key: " + key)
                return False
        return True

    if isinstance(golden, list):
        if not isinstance(generated, list):
            print("Generated is expected to be a list, but is not")
            return False

        for value in golden:
            if value not in generated:
                print("Generated file is missing value in list: " + str(value))
                return False
        return True

    return golden == generated


def main():
    args = parse_args()

    with open(args.generated, "r") as f:
        gen = json.load(f)
    with open(args.golden, "r") as f:
        golden = json.load(f)

    is_correct = False
    if args.subset:
        if is_subset(golden, gen):
            is_correct = True
    else:
        if sorting(gen) == sorting(golden):
            is_correct = True

    if not is_correct:
        print(
            "Comparison failure!. \n Golden:\n"
            + str(golden)
            + "\nGenerated:\n"
            + str(gen)
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
