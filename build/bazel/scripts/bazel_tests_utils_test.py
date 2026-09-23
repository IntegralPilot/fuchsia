#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))

import bazel_tests_utils
from build_utils import BazelPaths, MockCommandRunner


class BazelTestsUtilsTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self._td.name) / "fuchsia"
        self.fuchsia_dir.mkdir()
        (self.fuchsia_dir / ".jiri_manifest").write_text("")

        self.build_dir = self.fuchsia_dir / "out" / "build_dir"
        self.build_dir.mkdir(parents=True)

        BazelPaths.write_topdir_config_for_test(
            self.fuchsia_dir, "gen/build/bazel"
        )
        self.bazel_paths = BazelPaths(self.fuchsia_dir, self.build_dir)

        # Create the directories that BazelPaths properties expect
        self.bazel_paths.launcher.parent.mkdir(parents=True, exist_ok=True)
        self.bazel_paths.launcher.write_text("#!/bin/bash\nexit 0")
        self.bazel_paths.execroot.mkdir(parents=True, exist_ok=True)
        (self.bazel_paths.ninja_build_dir / "bazel_test_suites.txt").write_text(
            "//fake/test1\n//fake/test2"
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_generate_tests_json(self) -> None:
        mock_runner = MockCommandRunner()

        # Create some fake test info that matches what cquery would return
        test_info = {
            "label": "//src/my_test:my_test",
            "launcher_execroot_path": "bin/my_test",
            "runtime_deps_json_execroot_path": "bin/my_test.runtime_deps.json",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "",
        }

        mock_runner.push_result(stdout=json.dumps(test_info))

        tests_json, _ = bazel_tests_utils.generate_tests_json(
            self.bazel_paths, command_runner=mock_runner
        )

        self.assertEqual(len(tests_json), 1)
        entry = tests_json[0]
        self.assertEqual(entry["test"]["name"], "//src/my_test:my_test")
        self.assertEqual(entry["test"]["label"], "//src/my_test:my_test")
        self.assertEqual(entry["test"]["source_label"], "//src/my_test:my_test")

        # Verify path conversion
        # bazel_paths.execroot is fuchsia_dir/out/build_dir/gen/bazel/output_base/execroot/_main
        # path is relative to ninja_build_dir (fuchsia_dir/out/build_dir)
        expected_path = os.path.relpath(
            self.bazel_paths.execroot / "bin/my_test",
            self.bazel_paths.ninja_build_dir,
        )
        self.assertEqual(entry["test"]["path"], expected_path)

        expected_deps_path = os.path.relpath(
            self.bazel_paths.execroot / "bin/my_test.runtime_deps.json",
            self.bazel_paths.ninja_build_dir,
        )
        self.assertEqual(entry["test"]["runtime_deps"], expected_deps_path)

    def test_generate_tests_json_multiple_entries(self) -> None:
        mock_runner = MockCommandRunner()
        test1 = {
            "name": "test1",
            "label": "//t1",
            "source_label": "//t1",
            "launcher_execroot_path": "p1",
            "runtime_deps_json_execroot_path": "d1",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "",
        }
        test2 = {
            "name": "test2",
            "label": "//t2",
            "source_label": "//t2",
            "launcher_execroot_path": "p2",
            "runtime_deps_json_execroot_path": "d2",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "list_mock_unittests",
        }

        mock_runner.push_result(
            stdout=json.dumps(test1) + "\n" + json.dumps(test2)
        )

        tests_json, _ = bazel_tests_utils.generate_tests_json(
            self.bazel_paths, command_runner=mock_runner
        )

        self.assertEqual(len(tests_json), 2)
        self.assertEqual(tests_json[0]["test"]["name"], "//t1")
        self.assertEqual(tests_json[1]["test"]["name"], "//t2")

        execroot_path = "gen/build/bazel/output_base/execroot/_main"
        self.assertEqual(
            tests_json[0],
            {
                "environments": [
                    {
                        "dimensions": {
                            "os": "Linux",
                            "cpu": "x64",
                        }
                    }
                ],
                "expects_ssh": False,
                "test": {
                    "name": f"//t1",
                    "label": "//t1",
                    "source_label": "//t1",
                    "path": f"{execroot_path}/p1",
                    "runtime_deps": f"{execroot_path}/d1",
                    "os": "linux",
                    "cpu": "x64",
                },
            },
        )
        self.assertEqual(
            tests_json[1],
            {
                "environments": [
                    {
                        "dimensions": {
                            "os": "Linux",
                            "cpu": "x64",
                        }
                    }
                ],
                "expects_ssh": False,
                "test": {
                    "name": f"//t2",
                    "label": "//t2",
                    "source_label": "//t2",
                    "path": f"{execroot_path}/p2",
                    "runtime_deps": f"{execroot_path}/d2",
                    "os": "linux",
                    "cpu": "x64",
                    "list_cases_argument": "list_mock_unittests",
                },
            },
        )

    def test_generate_tests_json_failure(self) -> None:
        mock_runner = MockCommandRunner()
        mock_runner.push_result(returncode=1, stderr="Bazel error")

        with self.assertRaisesRegex(
            RuntimeError, "Failed to run bazel query: Bazel error"
        ):
            bazel_tests_utils.generate_tests_json(
                self.bazel_paths, command_runner=mock_runner
            )

    def test_generate_tests_json_missing_fuchsia_host_test_info(self) -> None:
        mock_runner = MockCommandRunner()
        missing_info = {
            "error": "missing_fuchsia_host_test_info",
            "label": "//tools/check-licenses:v2_config_test",
        }
        mock_runner.push_result(stdout=json.dumps(missing_info))

        with self.assertRaisesRegex(
            RuntimeError,
            r"Target '//tools/check-licenses:v2_config_test' included in the bazel_test_suites GN group is a test target but does not provide FuchsiaHostTestInfo",
        ):
            bazel_tests_utils.generate_tests_json(
                self.bazel_paths, command_runner=mock_runner
            )

    def test_generate_tests_json_multiple_missing_fuchsia_host_test_info(
        self,
    ) -> None:
        mock_runner = MockCommandRunner()
        missing_info1 = {
            "error": "missing_fuchsia_host_test_info",
            "label": "//tools/check-licenses:v2_config_test",
        }
        missing_info2 = {
            "error": "missing_fuchsia_host_test_info",
            "label": "//tools/whereiscl:whereiscl_test",
        }
        mock_runner.push_result(
            stdout=json.dumps(missing_info1) + "\n" + json.dumps(missing_info2)
        )

        with self.assertRaisesRegex(
            RuntimeError,
            r"The following targets included in the bazel_test_suites GN group are test targets but do not provide FuchsiaHostTestInfo:\n  - //tools/check-licenses:v2_config_test\n  - //tools/whereiscl:whereiscl_test",
        ):
            bazel_tests_utils.generate_tests_json(
                self.bazel_paths, command_runner=mock_runner
            )

    def test_generate_tests_json_with_debug_symbols(self) -> None:
        mock_runner = MockCommandRunner()
        test_info = {
            "label": "//src/my_test:my_test",
            "launcher_execroot_path": "bin/my_test",
            "runtime_deps_json_execroot_path": "bin/my_test.runtime_deps.json",
            "unstripped_binary_execroot_path": "bin/my_test_unstripped",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "",
        }
        mock_runner.push_result(stdout=json.dumps(test_info))

        bazel_tests_utils.generate_tests_json(
            self.bazel_paths, command_runner=mock_runner
        )

        manifest_file = (
            self.bazel_paths.ninja_build_dir
            / "bazel_host_tests.debug_symbols.json"
        )
        self.assertTrue(manifest_file.exists())
        execroot_path = "gen/build/bazel/output_base/execroot/_main"
        self.assertEqual(
            json.loads(manifest_file.read_text()),
            [
                {
                    "cpu": "x64",
                    "debug": f"{execroot_path}/bin/my_test_unstripped",
                    "label": "//src/my_test:my_test",
                    "os": "linux",
                }
            ],
        )


if __name__ == "__main__":
    unittest.main()
