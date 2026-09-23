#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import tempfile
import unittest

import generate_sdk_package_content_checklist
from parameterized import param, parameterized


class ConvertTest(unittest.TestCase):
    @parameterized.expand(
        [
            param(
                exit_code=0,
                manifest={
                    "version": "1",
                    "repository": "fuchsia.com",
                    "package": {"name": "foo", "version": "0"},
                    "blobs": [
                        {
                            "source_path": "path/to/meta.far",
                            "path": "meta/",
                            "merkle": "000",
                            "size": 1,
                        },
                        {
                            "source_path": "my/bar",
                            "path": "bin/bar",
                            "merkle": "111",
                            "size": 1,
                        },
                        {
                            "source_path": "my/baz",
                            "path": "bin/baz",
                            "merkle": "222",
                            "size": 1,
                        },
                        {
                            "source_path": "my/qux",
                            "path": "bin/qux",
                            "merkle": "333",
                            "size": 1,
                        },
                    ],
                },
                expected_files_exact=["meta/", "bin/bar"],
                expected_files_present=["my/baz"],
                reference={
                    "version": "1",
                    "content": {
                        "files": {
                            "meta/": {
                                "hash": "000",
                            },
                            "bin/bar": {
                                "hash": "111",
                            },
                            "bin/baz": {
                                "present": True,
                            },
                        }
                    },
                },
                warn=False,
            ),
            param(
                exit_code=1,
                manifest={
                    "version": "1",
                    "repository": "fuchsia.com",
                    "package": {"name": "foo", "version": "0"},
                    "blobs": [
                        {
                            "source_path": "path/to/meta.far",
                            "path": "meta/",
                            "merkle": "000",
                            "size": 1,
                        },
                        {
                            "source_path": "my/bar",
                            "path": "bin/bar",
                            "merkle": "111",
                            "size": 1,
                        },
                    ],
                },
                expected_files_exact=["bin/bar"],
                expected_files_present=["meta/"],
                reference={
                    "version": "1",
                    "content": {
                        "files": {
                            "meta/": {"present": True},
                            "bin/bar": {
                                "hash": "INCORRECT_HASH",
                            },
                        }
                    },
                },
                warn=False,
            ),
        ]
    )
    def test_run_main(
        self,
        exit_code,
        manifest,
        expected_files_exact,
        expected_files_present,
        reference,
        warn,
    ):
        with tempfile.TemporaryDirectory() as tmpdir:
            package_manifest_path = os.path.join(
                tmpdir, "package-manifest.json"
            )
            with open(package_manifest_path, "w") as file:
                file.write(json.dumps(manifest, indent=2))

            reference_path = os.path.join(
                tmpdir, "golden_content_checklist.api"
            )
            with open(reference_path, "w") as file:
                file.write(json.dumps(reference, indent=2))

            output_path = os.path.join(tmpdir, "content_checklist.api")
            sys.argv = [
                "",
                "--manifest",
                package_manifest_path,
                "--output",
                output_path,
                "--reference",
                reference_path,
            ]

            for expected_file in expected_files_exact:
                sys.argv += ["--expected-files-exact", expected_file]
            for expected_file in expected_files_present:
                sys.argv += ["--expected-files-present", expected_file]

            if warn:
                sys.argv += ["--warn"]

            result = generate_sdk_package_content_checklist.main()
            self.assertEqual(exit_code, result)
