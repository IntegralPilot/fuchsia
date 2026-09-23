# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
""" This script asserts that expected_metric_names_filepath refers to a file in
an allowlisted directory."""

import argparse
import os.path
import sys

ALLOWED_METRICS_DIR_PATHS = [
    "//src/tests/end_to_end/perf/expected_metric_names",
    "//vendor/google/tests/end_to_end/perf/expected_metric_names",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--expected-metric-names-filepath",
        help="Path to the expected metrics name file",
        nargs="*",
        default=[],
    )
    parser.add_argument(
        "--output-file", help="Path to the verification file", required=True
    )
    args = parser.parse_args()

    for metric_names_filepath in args.expected_metric_names_filepath:
        dir_path = os.path.dirname(metric_names_filepath)
        if not dir_path in ALLOWED_METRICS_DIR_PATHS:
            raise Exception(
                "Directory containing expected metric file must be one of: "
                f'"{ALLOWED_METRICS_DIR_PATHS}": {metric_names_filepath}'
            )

    with open(args.output_file, "w") as f:
        f.write("Verified!\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
