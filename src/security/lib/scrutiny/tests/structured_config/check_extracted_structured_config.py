# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import pathlib
import unittest

# NB: These must be kept in sync with the values in BUILD.gn.
component_name = "bar"
package_name = "for-test"
expected_value_in_policy = "check this string!"
expected_value_for_dont_check = "don't check this string!"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Check the report of configuration produced by scrutiny."
    )
    parser.add_argument(
        "--extracted-config",
        type=pathlib.Path,
        required=True,
        help="Path to JSON dump of structured configuration from scrutiny.",
    )
    args = parser.parse_args()

    test = unittest.TestCase()

    with open(args.extracted_config) as f:
        extracted_config = json.load(f)

    test.assertEqual(
        extracted_config[
            f"fuchsia-pkg://fuchsia.com/{package_name}#meta/{component_name}.cm"
        ],
        {
            "asserted_by_scrutiny_test": expected_value_in_policy,
            "mutable_by_parent_config": expected_value_for_dont_check,
        },
        "configuration from system image did not match expectation",
    )
