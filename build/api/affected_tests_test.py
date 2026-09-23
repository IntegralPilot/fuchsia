#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import affected_tests
import ninja_artifacts

sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../bazel/scripts"))
import re

from build_utils import (
    BazelPaths,
    CommandResult,
    MockBazelLauncher,
    MockCommandRunner,
    MockNinjaRunner,
)


class PartitioningMockBazelLauncher(MockBazelLauncher):
    """A mock BazelLauncher used to verify the binary partitioning algorithm
    used when determining which Bazel tests are affected by changed .bzl files.

    Usage is:
      1) Create instance, passing a mapping from test labels to the .bzl files
         they depend on.

      2) Pass the instance to the affected_tests.find_tests_affected_by_changed_files()
         function as the bazel_launcher argument.

      3) Check that the queries attribute contains the expected queries.
    """

    def __init__(self, target_to_bzl_map: dict[str, list[str]]) -> None:
        super().__init__()
        self.target_to_bzl_map = target_to_bzl_map
        self.queries: list[list[str]] = []

    def run_query(
        self, query_type: str, query_args: list[str], ignore_errors: bool
    ) -> CommandResult:
        self.queries.append(query_args)
        query_str = query_args[-1]
        match = re.search(r"set\((.*?)\)", query_str)
        if match:
            targets = match.group(1).split()
            bzl_files = set()
            for t in targets:
                bzl_files.update(self.target_to_bzl_map.get(t, []))
            return CommandResult(
                returncode=0, stdout="\n".join(sorted(bzl_files)), stderr=""
            )
        return CommandResult(returncode=0, stdout="", stderr="")


class CreateTestArtifactsMappingTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.root = Path(self._td.name)
        (self.root / ".jiri_manifest").write_text("")
        self.build_dir = self.root / "out/build"
        self.build_dir.mkdir(parents=True)
        self.tests_json_path = self.build_dir / "tests.json"
        BazelPaths.write_topdir_config_for_test(self.root, "bazel_topdir")

    def tearDown(self) -> None:
        self._td.cleanup()

    def write_json(self, path: Path, tests: T.Any) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("wt") as f:
            json.dump(tests, f)

    def test_no_tests(self) -> None:
        self.write_json(self.tests_json_path, [])
        result = affected_tests.create_gn_test_artifacts_mapping(self.build_dir)
        self.assertDictEqual(result, {})

    HOST_TEST_LABEL = "//src/microfuchsia:pkvm-hello-world-test(//build/toolchain/fuchsia:arm64)"
    HOST_TEST_RUNTIME_DEPS_PATH = (
        "gen/src/microfuchsia/pkvm-hello-world-test.host-arm64.deps.json"
    )
    HOST_TEST_RUNTIME_DEPS = [
        "arm64-shared/obj/sdk/fidl/fuchsia.gpu.virtio/fuchsia.gpu.virtio_bindlib/test_data/bind-tests/fuchsia.gpu.virtio.bind",
        "host_x64/seriallistener",
        "zbi-hello-world-test.host-arm64.sh",
    ]

    HOST_TEST_ENTRY = {
        "environments": [],
        "expects_ssh": False,
        "is_boot_test": True,
        "product_bundle": "pkvm-hello-world-test",
        "test": {
            "cpu": "arm64",
            "isolated": True,
            "label": HOST_TEST_LABEL,
            "log_settings": {"max_severity": "WARN"},
            "name": "pkvm-hello-world-test",
            "os": "linux",
            "path": "pkvm-hello-world-test.host-arm64.sh",
            "runtime_deps": HOST_TEST_RUNTIME_DEPS_PATH,
            "timeout_secs": 600,
        },
    }

    HOST_TEST_EXPECTED_SET = {
        "arm64-shared/obj/sdk/fidl/fuchsia.gpu.virtio/fuchsia.gpu.virtio_bindlib/test_data/bind-tests/fuchsia.gpu.virtio.bind",
        "host_x64/seriallistener",
        "pkvm-hello-world-test.host-arm64.sh",
        "zbi-hello-world-test.host-arm64.sh",
        HOST_TEST_RUNTIME_DEPS_PATH,
    }

    DEVICE_TEST_LABEL = "//src/power/testing/system-integration/example:bootstrap_pkg(//build/toolchain/fuchsia:arm64)"
    DEVICE_TEST_PACKAGE_MANIFEST_DEPS_PATH = "gen/src/power/testing/system-integration/example/bootstrap_pkg_test_bootstrap_component.pkg_manifests.json"
    DEVICE_TEST_PACKAGE_MANIFEST_DEPS = [
        "obj/src/developer/debug/debug_agent/debug_agent/package_manifest.json",
    ]

    DEVICE_TEST_RUNTIME_DEPS_PATH = "gen/src/power/testing/system-integration/example/bootstrap_pkg_test_bootstrap_component.deps.json"
    DEVICE_TEST_RUNTIME_DEPS = [
        "obj/src/devices/bind/fuchsia.test/fuchsia.test/test_data/bind-tests/fuchsia.test.bind",
        "host_x64/test-pilot",
        "bootstrap_power_system_integration_example_test_pkg_bootstrap_power_system_integration_example_test.cm_test.sh",
        "test_configs/bootstrap_power_system_integration_example_test_pkg.bootstrap_power_system_integration_example_test.cm.test_config.json",
    ]

    DEVICE_TEST_ENTRY = {
        "environments": [{"dimensions": {"device_type": "QEMU"}}],
        "expects_ssh": False,
        "test": {
            "build_rule": "fuchsia_bootfs_test_package",
            "component_label": "//src/power/testing/system-integration/example:bootstrap_component(//build/toolchain/fuchsia:arm64)",
            "cpu": "arm64",
            "label": DEVICE_TEST_LABEL,
            "log_settings": {"max_severity": "WARN"},
            "name": "fuchsia-boot:///bootstrap_power_system_integration_example_test_pkg#meta/bootstrap_power_system_integration_example_test.cm",
            "new_path": "bootstrap_power_system_integration_example_test_pkg_bootstrap_power_system_integration_example_test.cm_test.sh",
            "os": "fuchsia",
            "package_label": DEVICE_TEST_LABEL,
            "package_manifest_deps": DEVICE_TEST_PACKAGE_MANIFEST_DEPS_PATH,
            "package_manifests": [
                "obj/src/power/testing/system-integration/example/bootstrap_pkg/package_manifest.json"
            ],
            "package_url": "fuchsia-boot:///bootstrap_power_system_integration_example_test_pkg#meta/bootstrap_power_system_integration_example_test.cm",
            "runtime_deps": DEVICE_TEST_RUNTIME_DEPS_PATH,
        },
    }

    DEVICE_TEST_EXPECTED_SET = {
        "gen/src/power/testing/system-integration/example/bootstrap_pkg_test_bootstrap_component.pkg_manifests.json",
        "bootstrap_power_system_integration_example_test_pkg_bootstrap_power_system_integration_example_test.cm_test.sh",
        "gen/src/power/testing/system-integration/example/bootstrap_pkg_test_bootstrap_component.deps.json",
        "host_x64/test-pilot",
        "obj/src/developer/debug/debug_agent/debug_agent/package_manifest.json",
        "obj/src/devices/bind/fuchsia.test/fuchsia.test/test_data/bind-tests/fuchsia.test.bind",
        "obj/src/power/testing/system-integration/example/bootstrap_pkg/package_manifest.json",
        "test_configs/bootstrap_power_system_integration_example_test_pkg.bootstrap_power_system_integration_example_test.cm.test_config.json",
        DEVICE_TEST_RUNTIME_DEPS_PATH,
        DEVICE_TEST_PACKAGE_MANIFEST_DEPS_PATH,
    }

    def test_single_host_test(self) -> None:
        self.write_json(self.tests_json_path, [self.HOST_TEST_ENTRY])
        self.write_json(
            self.build_dir / self.HOST_TEST_RUNTIME_DEPS_PATH,
            self.HOST_TEST_RUNTIME_DEPS,
        )

        mapping = affected_tests.create_gn_test_artifacts_mapping(
            self.build_dir
        )
        self.assertEqual(len(mapping), 1)

        label, test_info = mapping.popitem()
        self.assertEqual(label, self.HOST_TEST_LABEL)
        self.assertEqual(test_info.os_name, "linux")
        self.assertSetEqual(
            test_info.ninja_artifacts, self.HOST_TEST_EXPECTED_SET
        )

    def test_single_device_test(self) -> None:
        self.write_json(self.tests_json_path, [self.DEVICE_TEST_ENTRY])
        self.write_json(
            self.build_dir / self.DEVICE_TEST_RUNTIME_DEPS_PATH,
            self.DEVICE_TEST_RUNTIME_DEPS,
        )
        self.write_json(
            self.build_dir / self.DEVICE_TEST_PACKAGE_MANIFEST_DEPS_PATH,
            self.DEVICE_TEST_PACKAGE_MANIFEST_DEPS,
        )

        mapping = affected_tests.create_gn_test_artifacts_mapping(
            self.build_dir
        )

        self.assertEqual(len(mapping), 1)

        target_label, test_info = mapping.popitem()
        self.assertEqual(target_label, self.DEVICE_TEST_LABEL)
        self.assertEqual(test_info.os_name, "fuchsia")
        self.assertSetEqual(
            test_info.ninja_artifacts, self.DEVICE_TEST_EXPECTED_SET
        )

    def test_multiple_tests(self) -> None:
        self.write_json(
            self.tests_json_path, [self.HOST_TEST_ENTRY, self.DEVICE_TEST_ENTRY]
        )
        self.write_json(
            self.build_dir / self.HOST_TEST_RUNTIME_DEPS_PATH,
            self.HOST_TEST_RUNTIME_DEPS,
        )
        self.write_json(
            self.build_dir / self.DEVICE_TEST_RUNTIME_DEPS_PATH,
            self.DEVICE_TEST_RUNTIME_DEPS,
        )
        self.write_json(
            self.build_dir / self.DEVICE_TEST_PACKAGE_MANIFEST_DEPS_PATH,
            self.DEVICE_TEST_PACKAGE_MANIFEST_DEPS,
        )

        mapping = affected_tests.create_gn_test_artifacts_mapping(
            self.build_dir
        )

        self.assertEqual(len(mapping), 2)

        target_label, test_info = mapping.popitem()
        self.assertEqual(target_label, self.DEVICE_TEST_LABEL)
        self.assertEqual(test_info.os_name, "fuchsia")
        self.assertSetEqual(
            test_info.ninja_artifacts, self.DEVICE_TEST_EXPECTED_SET
        )

        target_label, test_info = mapping.popitem()
        self.assertEqual(target_label, self.HOST_TEST_LABEL)
        self.assertEqual(test_info.os_name, "linux")
        self.assertSetEqual(
            test_info.ninja_artifacts, self.HOST_TEST_EXPECTED_SET
        )


class FindTestsAffectedByChangedFilesTest(unittest.TestCase):
    def setUp(self) -> None:
        self._td = tempfile.TemporaryDirectory()
        self.root = Path(self._td.name)
        (self.root / ".jiri_manifest").write_text("")
        self.build_dir = self.root / "out/build"
        self.build_dir.mkdir(parents=True)
        BazelPaths.write_topdir_config_for_test(self.root, "bazel_topdir")
        self.bazel_paths = BazelPaths.new(self.root, self.build_dir)
        self.bazel_paths.output_base.mkdir(parents=True)

        (
            self.build_dir / ninja_artifacts.NINJA_BUILD_PLAN_DEPS_FILE
        ).write_text(
            "build.ninja.stamp: ../../BUILD.gn ../../src/foo.gni dep1 dep2 dep3 dep4"
        )

        (self.root / "src/bazel").mkdir(parents=True)
        (self.root / "src/bazel/BUILD.bazel").touch()

        tests_json = [
            {
                "test": {
                    "label": "//gn:target1",
                    "path": "obj/gn/target1",
                    "os": "fuchsia",
                },
            },
            {
                "test": {
                    "label": "//bazel:target2",  # A Bazel test wrapped by a GN target.
                    "package_manifests": [
                        "obj/bazel/target2.bazel_outputs/package_manifest.json",
                    ],
                    "os": "linux",
                },
            },
            {
                "test": {
                    "label": "@@//src/bazel:target3",  # A Bazel test target.
                    "path": "bazel-bin/src/bazel/target3_bin",
                    "os": "linux",
                },
            },
        ]

        self.tests_json_path = self.build_dir / "tests.json"
        with self.tests_json_path.open("wt") as f:
            json.dump(tests_json, f)

    def tearDown(self) -> None:
        self._td.cleanup()

    def test_no_change(self) -> None:
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["some/file.txt"],
            self.root,
            MockNinjaRunner(self.build_dir, "obj/some/target2.out\n"),
            MockBazelLauncher.new_with_empty_outputs(),
        )
        self.assertSetEqual(targets, set())

    def test_one_target_affected(self) -> None:
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["gn/source.txt"],
            self.root,
            MockNinjaRunner(
                self.build_dir,
                "\n".join(["obj/gn/target1", "obj/gn/target1.o"]),
            ),
            MockBazelLauncher.new_with_empty_outputs(),
        )
        self.assertSetEqual(
            targets,
            {affected_tests.AffectedTestTarget("//gn:target1", "fuchsia")},
        )

        targets = affected_tests.find_tests_affected_by_changed_files(
            ["bazel/source.txt"],
            self.root,
            MockNinjaRunner(
                self.build_dir,
                "\n".join(
                    [
                        "obj/bazel/target2.bazel_outputs/foo",
                        "obj/bazel/target2.bazel_outputs/package_manifest.json",
                    ]
                ),
            ),
            MockBazelLauncher.new_with_empty_outputs(),
        )
        self.maxDiff = None
        self.assertSetEqual(
            targets,
            {affected_tests.AffectedTestTarget("//bazel:target2", "linux")},
        )

        def new_bazel_query_command_filter(
            queries: list[tuple[str, str]],
        ) -> T.Callable[[list[str]], CommandResult]:
            """Create a command filter for bazel queries performed by affected_test.py

            Args:
                queries: List of (query, expected_output) pairs.
            Returns:
                A new input value for MockCommandRunner.set_command_filter()
            """
            return MockCommandRunner.new_command_filter_from_list(
                [
                    (
                        f"bazel query --config=quiet --consistent_labels {q[0]} --keep_going",
                        q[1],
                    )
                    for q in queries
                ]
            )

        bazel_launcher = MockBazelLauncher()
        bazel_launcher.command_runner.set_command_filter(
            new_bazel_query_command_filter(
                [
                    (
                        "rdeps(//...,set(@@//:bazel/test3.cc))",
                        "@@//src/bazel:target3",
                    )
                ]
            )
        )

        targets = affected_tests.find_tests_affected_by_changed_files(
            ["bazel/test3.cc"],
            self.root,
            MockNinjaRunner(self.build_dir, ""),
            bazel_launcher,
        )
        self.maxDiff = None
        self.assertSetEqual(
            targets,
            {
                affected_tests.AffectedTestTarget(
                    "@@//src/bazel:target3", "linux"
                )
            },
        )

        targets = affected_tests.find_tests_affected_by_changed_files(
            ["bazel/source.txt", "gn/source.txt"],
            self.root,
            MockNinjaRunner(
                self.build_dir,
                "\n".join(
                    [
                        "obj/bazel/target2.bazel_outputs/foo",
                        "obj/bazel/target2.bazel_outputs/package_manifest.json",
                        "obj/gn/target1.o",
                        "obj/gn/target1",
                    ]
                ),
            ),
            MockBazelLauncher.new_with_empty_outputs(),
        )
        self.assertSetEqual(
            targets,
            {
                affected_tests.AffectedTestTarget("//gn:target1", "fuchsia"),
                affected_tests.AffectedTestTarget("//bazel:target2", "linux"),
            },
        )

    def test_native_bazel_target_affected(self) -> None:
        tests_json = [
            {
                "test": {
                    "label": "@@//src/bazel:test1",
                    "os": "linux",
                },
            },
            {
                "test": {
                    "label": "@@//src/bazel:test2",
                    "os": "linux",
                }
            },
        ]
        with self.tests_json_path.open("wt") as f:
            json.dump(tests_json, f)

        mock_ninja_runner = MockNinjaRunner(self.build_dir, "")

        mock_bazel_launcher = MockBazelLauncher()
        mock_bazel_launcher.push_expected_outputs(
            [
                # Result of rdeps(deps(set(//src/bazel:test1.cc))) query
                "@@//src/bazel:test1\n",
            ]
        )

        # First, check that if the source of only one test is modified, only that specific
        # test is reported.
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/bazel/test1.cc"],
            self.root,
            mock_ninja_runner,
            mock_bazel_launcher,
        )

        self.assertSetEqual(
            targets,
            {affected_tests.AffectedTestTarget("@@//src/bazel:test1", "linux")},
        )

        # Do the same for the second test.
        mock_bazel_launcher.push_expected_outputs(
            [
                # Result of rdeps(deps(set(//src/bazel:test2.cc))) query
                "@@//src/bazel:test2\n",
            ]
        )

        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/bazel/test2.cc"],
            self.root,
            mock_ninja_runner,
            mock_bazel_launcher,
        )
        self.assertSetEqual(
            targets,
            {affected_tests.AffectedTestTarget("@@//src/bazel:test2", "linux")},
        )

        # Do the same for a build file.
        mock_bazel_launcher.push_expected_outputs(
            [
                # Result of rdeps(deps(set(//src/bazel:all))) query
                "@@//src/bazel:test1\n"
                + "@@//src/bazel:test2\n",
            ]
        )
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/bazel/BUILD.bazel"],
            self.root,
            mock_ninja_runner,
            mock_bazel_launcher,
        )
        self.assertSetEqual(
            targets,
            {
                affected_tests.AffectedTestTarget(
                    "@@//src/bazel:test1", "linux"
                ),
                affected_tests.AffectedTestTarget(
                    "@@//src/bazel:test2", "linux"
                ),
            },
        )

    def test_bzl_file_changes_binary_partitioning(self) -> None:
        tests_json = [
            {
                "test": {
                    "label": "@@//src/bazel:test1",
                    "os": "linux",
                },
            },
            {
                "test": {
                    "label": "@@//src/bazel:test2",
                    "os": "linux",
                }
            },
            {
                "test": {
                    "label": "@@//src/bazel:test3",
                    "os": "linux",
                }
            },
            {
                "test": {
                    "label": "@@//src/bazel:test4",
                    "os": "linux",
                }
            },
        ]
        with self.tests_json_path.open("wt") as f:
            json.dump(tests_json, f)

        mock_ninja_runner = MockNinjaRunner(self.build_dir, "")

        target_to_bzl_map: dict[str, list[str]] = {
            "@@//src/bazel:test1": [],
            "@@//src/bazel:test2": ["@@//src/bazel:foo.bzl"],
            "@@//src/bazel:test3": [],
            "@@//src/bazel:test4": [],
        }

        mock_bazel_launcher = PartitioningMockBazelLauncher(target_to_bzl_map)

        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/bazel/foo.bzl"],
            self.root,
            mock_ninja_runner,
            mock_bazel_launcher,
        )

        self.assertSetEqual(
            targets,
            {affected_tests.AffectedTestTarget("@@//src/bazel:test2", "linux")},
        )

        self.assertEqual(len(mock_bazel_launcher.queries), 5)

        def get_query_str(args: list[str]) -> str:
            return args[-1]

        self.assertIn(
            "set(@@//src/bazel:test1 @@//src/bazel:test2 @@//src/bazel:test3 @@//src/bazel:test4)",
            get_query_str(mock_bazel_launcher.queries[0]),
        )
        self.assertIn(
            "set(@@//src/bazel:test3 @@//src/bazel:test4)",
            get_query_str(mock_bazel_launcher.queries[1]),
        )
        self.assertIn(
            "set(@@//src/bazel:test1 @@//src/bazel:test2)",
            get_query_str(mock_bazel_launcher.queries[2]),
        )
        self.assertIn(
            "set(@@//src/bazel:test2)",
            get_query_str(mock_bazel_launcher.queries[3]),
        )
        self.assertIn(
            "set(@@//src/bazel:test1)",
            get_query_str(mock_bazel_launcher.queries[4]),
        )

    def test_gn_label_to_build_gn_path(self) -> None:
        self.assertEqual(
            affected_tests.gn_label_to_build_gn_path(
                "//src/foo/bar:bar(//build/toolchain/fuchsia:x64)"
            ),
            "src/foo/bar/BUILD.gn",
        )
        self.assertEqual(
            affected_tests.gn_label_to_build_gn_path("//src/foo/bar:bar"),
            "src/foo/bar/BUILD.gn",
        )
        self.assertEqual(
            affected_tests.gn_label_to_build_gn_path("//src/foo"),
            "src/foo/BUILD.gn",
        )
        self.assertEqual(
            affected_tests.gn_label_to_build_gn_path("//:root_target"),
            "BUILD.gn",
        )
        self.assertEqual(
            affected_tests.gn_label_to_build_gn_path("@@//src/foo:bar"),
            "",
        )
        self.assertEqual(
            affected_tests.gn_label_to_build_gn_path(""),
            "",
        )

    def test_gn_test_affected_by_build_gn(self) -> None:
        tests_json = [
            {
                "test": {
                    "label": "//src/foo:foo_test(//build/toolchain:x64)",
                    "package_label": "//src/foo:foo_pkg(//build/toolchain:x64)",
                    "os": "fuchsia",
                },
            },
            {
                "test": {
                    "label": "//src/bar/sub:bar_test(//build/toolchain:x64)",
                    "package_label": "//src/bar/pkg:bar_pkg(//build/toolchain:x64)",
                    "os": "linux",
                },
            },
        ]
        with self.tests_json_path.open("wt") as f:
            json.dump(tests_json, f)

        # MockNinjaRunner returns only build.ninja.stamp as ninja -t affected does for BUILD.gn
        def new_mock_ninja_runner() -> MockNinjaRunner:
            return MockNinjaRunner(self.build_dir, "build.ninja.stamp\n")

        mock_bazel_launcher = MockBazelLauncher.new_with_empty_outputs()

        # 1. Modifying the test's target BUILD.gn marks it as affected
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/foo/BUILD.gn"],
            self.root,
            new_mock_ninja_runner(),
            mock_bazel_launcher,
        )
        self.assertSetEqual(
            targets,
            {
                affected_tests.AffectedTestTarget(
                    "//src/foo:foo_test(//build/toolchain:x64)", "fuchsia"
                )
            },
        )

        # 2. Modifying the test package's BUILD.gn marks it as affected
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/bar/pkg/BUILD.gn"],
            self.root,
            new_mock_ninja_runner(),
            mock_bazel_launcher,
        )
        self.assertSetEqual(
            targets,
            {
                affected_tests.AffectedTestTarget(
                    "//src/bar/sub:bar_test(//build/toolchain:x64)", "linux"
                )
            },
        )

        # 3. Modifying secondary overlay BUILD.gn marks it as affected
        tests_json_secondary = [
            {
                "test": {
                    "label": "//third_party/libfoo:libfoo_test(//build/toolchain:x64)",
                    "os": "linux",
                },
            }
        ]
        with self.tests_json_path.open("wt") as f:
            json.dump(tests_json_secondary, f)

        targets = affected_tests.find_tests_affected_by_changed_files(
            ["build/secondary/third_party/libfoo/BUILD.gn"],
            self.root,
            new_mock_ninja_runner(),
            mock_bazel_launcher,
        )
        self.assertSetEqual(
            targets,
            {
                affected_tests.AffectedTestTarget(
                    "//third_party/libfoo:libfoo_test(//build/toolchain:x64)",
                    "linux",
                )
            },
        )

        # 4. Modifying unrelated BUILD.gn does not mark any test as affected
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/unrelated/BUILD.gn"],
            self.root,
            new_mock_ninja_runner(),
            mock_bazel_launcher,
        )
        self.assertSetEqual(targets, set())

        # 5. Modifying only non-BUILD.gn files does not match BUILD.gn logic
        targets = affected_tests.find_tests_affected_by_changed_files(
            ["src/foo/some_file.cc"],
            self.root,
            new_mock_ninja_runner(),
            mock_bazel_launcher,
        )
        self.assertSetEqual(targets, set())


if __name__ == "__main__":
    unittest.main()
