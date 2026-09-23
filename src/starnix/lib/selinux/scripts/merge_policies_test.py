#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import pathlib
import tempfile
import unittest

import merge_policies


class FastbootTests(unittest.TestCase):
    TESTDATA_DIR = None

    def test_merge_success(self) -> None:
        initial_sids_path = f"{FastbootTests.TESTDATA_DIR}/initial_sids"
        a_policy_path = f"{FastbootTests.TESTDATA_DIR}/a_policy.conf"
        b_policy_path = f"{FastbootTests.TESTDATA_DIR}/b_policy.conf"
        golden_policy_path = (
            f"{FastbootTests.TESTDATA_DIR}/golden_a_b_policy.conf"
        )
        with tempfile.TemporaryDirectory() as temporary_directory_name:
            merged_policy_path = f"{temporary_directory_name}/policy.conf"
            merge_policies.merge_text_policies(
                initial_sids_path,
                [a_policy_path, b_policy_path],
                merged_policy_path,
            )
            with open(merged_policy_path) as merged_policy, open(
                golden_policy_path
            ) as golden_policy:
                self.assertSequenceEqual(
                    tuple(merged_policy), tuple(golden_policy)
                )

    def test_merge_failure(self) -> None:
        initial_sids_path = f"{FastbootTests.TESTDATA_DIR}/initial_sids"
        a_policy_path = f"{FastbootTests.TESTDATA_DIR}/a_policy.conf"
        invalid_policy_path = (
            f"{FastbootTests.TESTDATA_DIR}/invalid_policy.conf"
        )

        with tempfile.TemporaryDirectory() as temporary_directory_name:
            merged_policy_path = f"{temporary_directory_name}/policy.conf"
            with self.assertRaises(ValueError) as context:
                merge_policies.merge_text_policies(
                    initial_sids_path,
                    [a_policy_path, invalid_policy_path],
                    merged_policy_path,
                )

            self.assertIn("Expected policy with", str(context.exception))
            self.assertIn(
                "but filtered policy contains", str(context.exception)
            )

    def test_extract_statements(self) -> None:
        policy = """\
allow s0 s1: file read;
if (secure_mode) { # comment with { brace
    allow s0 s1: file write;
} else {
    deny s0 s1: file write;
}
allow s0 s2: file read;
if (single_line) { # Block continues until }
    allow s0 s3: file write;
}
if (single_line_inline) { allow s0 s4: file exec; }
"""
        statements = merge_policies.extract_statements(policy)
        self.assertEqual(len(statements), 5)
        self.assertEqual(statements[0], "allow s0 s1: file read;")
        self.assertEqual(
            statements[1],
            "if (secure_mode) { # comment with { brace\n"
            "    allow s0 s1: file write;\n"
            "} else {\n"
            "    deny s0 s1: file write;\n"
            "}",
        )
        self.assertEqual(statements[2], "allow s0 s2: file read;")
        self.assertEqual(
            statements[3],
            "if (single_line) { # Block continues until }\n"
            "    allow s0 s3: file write;\n"
            "}",
        )
        self.assertEqual(
            statements[4],
            "if (single_line_inline) { allow s0 s4: file exec; }",
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--testdata-dir",
        required=True,
        type=pathlib.Path,
        help="Path to testdata",
    )
    args = parser.parse_args()
    FastbootTests.TESTDATA_DIR = args.testdata_dir
    unittest.main(argv=["-v"])


if __name__ == "__main__":
    main()
