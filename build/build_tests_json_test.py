#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit-tests for build/build_tests_json.py functions."""

import json
import os
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import build_tests_json
from build_utils import BazelPaths, CommandRunner, MockCommandRunner


class BuildTestsJsonTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self._dir = Path(self._td.name)
        self.source_dir = self._dir / "source"
        self.source_dir.mkdir()
        (self.source_dir / ".jiri_manifest").touch()

        self.build_dir = self.source_dir / "out" / "not-default"
        self.build_dir.mkdir(parents=True)

        self.output_dir = self._dir / "output"
        self.output_dir.mkdir()

        (self.build_dir / "obj" / "tests").mkdir(parents=True)

        # Compute the Bazel execroot path, relative to the build directory.
        BazelPaths.write_topdir_config_for_test(self.source_dir, "bazel_topdir")
        self.execroot_path = os.path.relpath(
            BazelPaths(self.source_dir, self.build_dir).execroot,
            self.build_dir,
        )

    def tearDown(self) -> None:
        self._td.cleanup()

    def _test(
        self,
        tests_from_metadata: list[T.Any],
        test_groups: list[T.Any],
        product_bundles: list[T.Any],
        with_bazel_tests: bool = False,
        command_runner: CommandRunner | None = None,
    ) -> tuple[set[Path], list[T.Any]]:
        tests_from_metadata_str = json.dumps(tests_from_metadata)
        tests_from_metadata_path = self.build_dir / "tests_from_metadata.json"
        tests_from_metadata_path.write_text(tests_from_metadata_str)

        test_groups_str = json.dumps(test_groups)
        test_groups_path = (
            self.build_dir / "obj" / "tests" / "product_bundle_test_groups.json"
        )
        test_groups_path.write_text(test_groups_str)

        product_bundles_str = json.dumps(product_bundles)
        product_bundles_path = self.build_dir / "product_bundles.json"
        product_bundles_path.write_text(product_bundles_str)

        if with_bazel_tests:
            (self.build_dir / "bazel_test_suites.txt").write_text(
                "//fake/test1\n//fake/test2"
            )
        inputs = build_tests_json.build_tests_json(
            self.build_dir, with_bazel_tests, command_runner
        )

        tests_string = (self.build_dir / "tests.json").read_text()
        tests = json.loads(tests_string)

        return (inputs, tests)

    def test_only_tests_from_metadata_no_environments(self) -> None:
        tests_from_metadata = [
            {"test": {"name": "test1"}},
            {"test": {"name": "test2"}},
        ]
        product_bundles = [{"name": "my_pb"}]
        (_, tests) = self._test(tests_from_metadata, [], product_bundles)
        self.assertEqual(tests_from_metadata, tests)

    def test_only_test_groups(self) -> None:
        tests_json = [{"test": {"name": "test1"}}, {"test": {"name": "test2"}}]
        tests_json_str = json.dumps(tests_json)
        tests_json_path = self.build_dir / "pb_tests.json"
        tests_json_path.write_text(tests_json_str)

        test_groups = [
            {"product_bundle_name": "my_pb", "tests_json": str(tests_json_path)}
        ]
        product_bundles = [{"name": "my_pb"}]
        (_, tests) = self._test([], test_groups, product_bundles)

        expected_tests_json = [
            {"product_bundle": "my_pb", "test": {"name": "test1-my_pb"}},
            {"product_bundle": "my_pb", "test": {"name": "test2-my_pb"}},
        ]
        self.assertEqual(expected_tests_json, tests)

    def test_incorrect_product_bundle_name(self) -> None:
        tests_json = [{"test": {"name": "test1"}}, {"test": {"name": "test2"}}]
        tests_json_str = json.dumps(tests_json)
        tests_json_path = self.build_dir / "pb_tests.json"
        tests_json_path.write_text(tests_json_str)

        test_groups = [
            {
                "product_bundle_name": "my_pb_incorrect",
                "tests_json": str(tests_json_path),
            }
        ]
        product_bundles = [{"name": "my_pb"}]

        with self.assertRaises(SystemExit):
            self._test([], test_groups, product_bundles)

    def test_metadata_and_product_bundles(self) -> None:
        tests_from_metadata = [
            {"test": {"name": "test1"}},
            {"test": {"name": "test2"}},
        ]
        tests_json = [{"test": {"name": "test1"}}, {"test": {"name": "test2"}}]
        tests_json_str = json.dumps(tests_json)
        tests_json_path = self.build_dir / "pb_tests.json"
        tests_json_path.write_text(tests_json_str)
        product_bundles = [{"name": "my_pb"}]

        env = {"dimensions": {"device_type": "Vim3"}}
        test_groups = [
            {
                "product_bundle_name": "my_pb",
                "environments": [env],
                "tests_json": str(tests_json_path),
            }
        ]

        _, tests = self._test(
            tests_from_metadata,
            test_groups,
            product_bundles,
        )

        expected_tests_json = [
            {"test": {"name": "test1"}},
            {"test": {"name": "test2"}},
            {
                "product_bundle": "my_pb",
                "environments": [env],
                "test": {"name": "test1-my_pb"},
            },
            {
                "product_bundle": "my_pb",
                "environments": [env],
                "test": {"name": "test2-my_pb"},
            },
        ]
        self.assertEqual(expected_tests_json, tests)

    def test_bazel_tests(self) -> None:
        # Prepare mock runner for bazel cquery
        mock_runner = MockCommandRunner()
        test1 = {
            "name": "test1",
            "label": "@@//t1",
            "source_label": "@@//t1",
            "launcher_execroot_path": "p1",
            "runtime_deps_json_execroot_path": "d1",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "list_cases_1",
        }
        test2 = {
            "name": "test2",
            "label": "@@//t2",
            "source_label": "@@//t2",
            "launcher_execroot_path": "p2",
            "runtime_deps_json_execroot_path": "d2",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "",
        }
        mock_runner.push_result(
            stdout=json.dumps(test1) + "\n" + json.dumps(test2)
        )

        dummy_host_test = {
            "environments": [
                {
                    "dimensions": {
                        "os": "Linux",
                        "cpu": "x64",
                    }
                }
            ],
            "test": {
                "name": "dummy_host_test",
            },
        }
        _, tests = self._test(
            [dummy_host_test],
            [],
            [],
            with_bazel_tests=True,
            command_runner=mock_runner,
        )

        expected_tests_json: list[dict[str, T.Any]] = [
            dummy_host_test,
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
                    "name": "//t1",
                    "label": "@@//t1",
                    "source_label": "//t1",
                    "path": f"{self.execroot_path}/p1",
                    "runtime_deps": f"{self.execroot_path}/d1",
                    "os": "linux",
                    "cpu": "x64",
                    "list_cases_argument": "list_cases_1",
                },
            },
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
                    "name": "//t2",
                    "label": "@@//t2",
                    "source_label": "//t2",
                    "path": f"{self.execroot_path}/p2",
                    "runtime_deps": f"{self.execroot_path}/d2",
                    "os": "linux",
                    "cpu": "x64",
                },
            },
        ]
        self.assertEqual(len(expected_tests_json), len(tests))
        self.assertDictEqual(expected_tests_json[0], tests[0])
        self.assertDictEqual(expected_tests_json[1], tests[1])

    def test_full(self) -> None:
        tests_from_metadata = [
            {
                "test": {"name": "test1"},
                "environments": [{"dimensions": {"os": "Linux"}}],
            },
            {"test": {"name": "test2"}},
        ]
        tests_json = [{"test": {"name": "test1"}}, {"test": {"name": "test2"}}]
        tests_json_str = json.dumps(tests_json)
        tests_json_path = self.build_dir / "pb_tests.json"
        tests_json_path.write_text(tests_json_str)
        product_bundles = [{"name": "my_pb"}]

        env = {"dimensions": {"device_type": "Vim3"}}
        test_groups = [
            {
                "product_bundle_name": "my_pb",
                "environments": [env],
                "tests_json": str(tests_json_path),
            }
        ]

        mock_runner = MockCommandRunner()
        test1 = {
            "name": "test1",
            "label": "@@//t1",
            "source_label": "@@//t1",
            "launcher_execroot_path": "p1",
            "runtime_deps_json_execroot_path": "d1",
            "os": "linux",
            "cpu": "x64",
            "list_cases_argument": "",
        }
        mock_runner.push_result(stdout=json.dumps(test1))

        (_, tests) = self._test(
            tests_from_metadata,
            test_groups,
            product_bundles,
            with_bazel_tests=True,
            command_runner=mock_runner,
        )

        expected_tests_json: list[dict[str, T.Any]] = [
            {
                "test": {"name": "test1"},
                "environments": [{"dimensions": {"os": "Linux"}}],
            },
            {"test": {"name": "test2"}},
            {
                "product_bundle": "my_pb",
                "environments": [env],
                "test": {"name": "test1-my_pb"},
            },
            {
                "product_bundle": "my_pb",
                "environments": [env],
                "test": {"name": "test2-my_pb"},
            },
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
                    "name": "//t1",
                    "label": "@@//t1",
                    "source_label": "//t1",
                    "path": f"{self.execroot_path}/p1",
                    "runtime_deps": f"{self.execroot_path}/d1",
                    "os": "linux",
                    "cpu": "x64",
                },
            },
        ]
        self.assertEqual(expected_tests_json, tests)

    def test_ninja_inputs(self) -> None:
        (inputs, _) = self._test([], [], [])
        self.assertEqual(
            {
                Path(self.build_dir / "tests_from_metadata.json"),
                Path(
                    self.build_dir
                    / "obj"
                    / "tests"
                    / "product_bundle_test_groups.json"
                ),
            },
            inputs,
        )

    def test_only_write_if_changed(self) -> None:
        tests_from_metadata = [
            {"test": {"name": "test1"}},
            {"test": {"name": "test2"}},
        ]
        tests_json = json.dumps(tests_from_metadata)
        tests_json_path = self.build_dir / "tests.json"
        tests_json_path.write_text(tests_json)
        previous_write_time = os.path.getmtime(tests_json_path)

        self._test(tests_from_metadata, [], [])

        # ensure the file did not change
        current_write_time = os.path.getmtime(tests_json_path)
        self.assertEqual(previous_write_time, current_write_time)


if __name__ == "__main__":
    unittest.main()
