#!/usr/bin/env fuchsia-vendored-python

# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import difflib
import json
import os
import sys


def by_name(x):
    return x["name"]


def normalize_entry(entry):
    entry["package_breakdown"] = sorted(
        list(entry["package_breakdown"].values()), key=by_name
    )
    for pb in entry["package_breakdown"]:
        # Contents of blobs are verified in other build steps, in size reports we
        # only verify size consistency to reduce noise.
        for blob in pb["blobs"]:
            blob.pop("merkle")
        pb["blobs"].sort(key=lambda x: x["path_in_package"])
    return entry


def normalize(size_report):
    return sorted(
        [normalize_entry(entry) for entry in size_report.values()], key=by_name
    )


def main():
    parser = argparse.ArgumentParser(description="Compares size reports")
    parser.add_argument(
        "--verbose_size_report1", type=argparse.FileType("r"), required=True
    )
    parser.add_argument(
        "--verbose_size_report2", type=argparse.FileType("r"), required=True
    )
    parser.add_argument("--normalized_report_output1", required=True)
    parser.add_argument("--normalized_report_output2", required=True)
    parser.add_argument("--diff_output", required=True)

    args = parser.parse_args()

    size_report1 = normalize(json.load(args.verbose_size_report1))
    size_report2 = normalize(json.load(args.verbose_size_report2))

    norm1 = json.dumps(size_report1, sort_keys=True, indent=2)
    norm2 = json.dumps(size_report2, sort_keys=True, indent=2)
    with open(args.normalized_report_output1, "w") as normalized_report_output1:
        normalized_report_output1.write(norm1)
    with open(args.normalized_report_output2, "w") as normalized_report_output2:
        normalized_report_output2.write(norm2)

    diff = difflib.unified_diff(
        norm1.splitlines(),
        norm2.splitlines(),
        args.verbose_size_report1.name,
        args.verbose_size_report2.name,
        lineterm="",
    )
    diffstr = "\n".join(diff)
    with open(args.diff_output, "w") as diff_output:
        diff_output.write(diffstr)

    if len(diffstr) != 0:
        print(
            f"Error: non-empty diff product configs:\n{diffstr}\n",
            file=sys.stderr,
        )

        diff_output_path = os.path.abspath(args.diff_output.name)
        print(
            f"This diff can also be found in file {diff_output_path}\n",
            file=sys.stderr,
        )

        norm1_path = os.path.abspath(args.normalized_report_output1.name)
        norm2_path = os.path.abspath(args.normalized_report_output2.name)
        print(
            "This diff is based on the following normalized reports:\n\n"
            f"  {norm1_path}\n"
            f"  {norm2_path}\n",
            file=sys.stderr,
        )

        return 1


if __name__ == "__main__":
    sys.exit(main())
