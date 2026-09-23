#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit-tests for build/regenerator.py functions."""

import os
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

# Import regenerator.py as a module.
_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import regenerator


class ContentHashTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._dir = Path(self._td.name)
        self.source_dir = self._dir / "source"
        self.source_dir.mkdir()

        self.build_dir = self.source_dir / "out" / "not-default"
        self.build_dir.mkdir(parents=True)

        self.output_dir = self._dir / "output"
        self.output_dir.mkdir()

        (self.source_dir / "foo.txt").write_text("FOO")
        (self.source_dir / "subdir").mkdir()
        (self.source_dir / "subdir" / "blob").write_bytes(b"012345678")
        (self.source_dir / "subdir" / "script.sh").write_text(
            "#!/bin/sh\necho 42\n"
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def _test(
        self,
        content_hashes: list[dict[str, T.Any]],
        expected_input_paths: list[Path],
        expected_hash_filenames: list[str],
    ) -> None:
        input_paths = regenerator.generate_bazel_content_hash_files(
            self.source_dir, self.build_dir, self.output_dir, content_hashes
        )

        self.assertListEqual(sorted(input_paths), expected_input_paths)

        hash_filenames = sorted(os.listdir(self.output_dir))
        self.assertListEqual(hash_filenames, expected_hash_filenames)

    def test_generate_bazel_content_hash_files(self) -> None:
        _TEST_CASES: list[dict[str, T.Any]] = [
            {
                "name": "empty",
                "content_hashes": [],
                "expected_inputs": [],
                "expected_hash_filenames": [],
            },
            {
                "name": "single_file",
                "content_hashes": [
                    {
                        "source_paths": ["//foo.txt"],
                        "repo_name": "foo_repo",
                    }
                ],
                "expected_inputs": [self.source_dir / "foo.txt"],
                "expected_hash_filenames": ["foo_repo.hash"],
            },
            {
                "name": "single_directory",
                "content_hashes": [
                    {
                        "source_paths": ["//subdir"],
                        "repo_name": "subdir_repo",
                    }
                ],
                "expected_inputs": [
                    self.source_dir / "subdir" / "blob",
                    self.source_dir / "subdir" / "script.sh",
                ],
                "expected_hash_filenames": ["subdir_repo.hash"],
            },
            {
                "name": "file_and_directory",
                "content_hashes": [
                    {
                        "source_paths": ["//foo.txt", "//subdir"],
                        "repo_name": "file_and_subdir_repo",
                    }
                ],
                "expected_inputs": [
                    self.source_dir / "foo.txt",
                    self.source_dir / "subdir" / "blob",
                    self.source_dir / "subdir" / "script.sh",
                ],
                "expected_hash_filenames": ["file_and_subdir_repo.hash"],
            },
            {
                "name": "exclude_suffixes",
                "content_hashes": [
                    {
                        "source_paths": ["//foo.txt", "//subdir"],
                        "repo_name": "exclude_suffixes",
                        "exclude_suffixes": [".sh"],
                    }
                ],
                "expected_inputs": [
                    self.source_dir / "foo.txt",
                    self.source_dir / "subdir" / "blob",
                ],
                "expected_hash_filenames": ["exclude_suffixes.hash"],
            },
            {
                "name": "multiple_entries",
                "content_hashes": [
                    {
                        "source_paths": ["//foo.txt"],
                        "repo_name": "foo",
                    },
                    {
                        "source_paths": ["//subdir"],
                        "repo_name": "subdir",
                    },
                ],
                "expected_inputs": [
                    self.source_dir / "foo.txt",
                    self.source_dir / "subdir" / "blob",
                    self.source_dir / "subdir" / "script.sh",
                ],
                "expected_hash_filenames": ["foo.hash", "subdir.hash"],
            },
        ]
        for test_case in _TEST_CASES:
            output_dir = self.output_dir / test_case["name"]
            output_dir.mkdir()

            input_paths = regenerator.generate_bazel_content_hash_files(
                self.source_dir,
                self.build_dir,
                output_dir,
                test_case["content_hashes"],
            )

            msg = f"For {test_case['name']} case."
            self.assertListEqual(
                sorted(input_paths), test_case["expected_inputs"], msg=msg
            )

            hash_filenames = sorted(os.listdir(output_dir))
            self.assertListEqual(
                hash_filenames, test_case["expected_hash_filenames"], msg=msg
            )

    def test_interpret_gn_path(self) -> None:
        _TEST_CASES: list[dict[str, T.Any]] = [
            {
                "name": "source path",
                "gn_path": "//src/foo",
                "expected": self.source_dir / "src" / "foo",
            },
            {
                "name": "absolute path",
                "gn_path": str(self.source_dir / "absolute"),
                "expected": (self.source_dir / "absolute").resolve(),
            },
            {
                "name": "relative dir",
                "gn_path": "gen/foo.stamp",
                "expected": self.build_dir / "gen" / "foo.stamp",
            },
        ]
        for test_case in _TEST_CASES:
            self.assertEqual(
                regenerator.interpret_gn_path(
                    test_case["gn_path"], self.source_dir, self.build_dir
                ),
                test_case["expected"],
                msg=test_case["name"],
            )


if __name__ == "__main__":
    unittest.main()
