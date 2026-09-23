#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import itertools
import os
import sys
import unittest
from pathlib import Path
from unittest import mock

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)

import bazel_rust_analyzer_utils
from bazel_rust_analyzer_utils import (
    Crate,
    CrateSpec,
    CrateSpecBuild,
    CrateSpecSource,
)


class TestBazelRustAnalyzerUtils(unittest.TestCase):
    @mock.patch("build_utils.BazelPaths")
    def test_substitute_tokens(self, MockBazelPaths: mock.Mock) -> None:
        mock_paths = MockBazelPaths()
        mock_paths.fuchsia_dir = Path("/fuchsia_dir")
        mock_paths.workspace = Path("/workspace")
        mock_paths.execroot = Path("/execroot")
        mock_paths.output_base = Path("/output_base")

        test_cases = [
            ("Hello __WORKSPACE__", "Hello /fuchsia_dir"),
            ("Path is ${pwd}/foo", "Path is /execroot/foo"),
            ("Exec root: __EXEC_ROOT__", "Exec root: /execroot"),
            ("Output base: __OUTPUT_BASE__", "Output base: /output_base"),
            (
                "Combined: __WORKSPACE__ ${pwd} __EXEC_ROOT__ __OUTPUT_BASE__",
                "Combined: /fuchsia_dir /execroot /execroot /output_base",
            ),
            ("No tokens here", "No tokens here"),
        ]

        for input_str, expected_str in test_cases:
            with self.subTest(input_str=input_str):
                result = bazel_rust_analyzer_utils.substitute_tokens(
                    input_str, mock_paths
                )
                self.assertEqual(result, expected_str)

    @mock.patch("build_utils.BazelPaths")
    @mock.patch("pathlib.Path.read_text")
    def test_load_crate_spec_from_json_empty(
        self, mock_read_text: mock.Mock, MockBazelPaths: mock.Mock
    ) -> None:
        mock_paths = MockBazelPaths()
        mock_read_text.return_value = "{}"
        result = bazel_rust_analyzer_utils.load_crate_spec_from_json(
            Path("_unused_dummy.json"), mock_paths
        )
        self.assertEqual(result, {})

    @mock.patch("build_utils.BazelPaths")
    @mock.patch("pathlib.Path.read_text")
    def test_load_crate_spec_from_json(
        self, mock_read_text: mock.Mock, MockBazelPaths: mock.Mock
    ) -> None:
        mock_paths = MockBazelPaths()
        mock_paths.fuchsia_dir = Path("/fuchsia_dir")
        mock_paths.workspace = Path("/workspace")
        mock_paths.execroot = Path("/execroot")
        mock_paths.output_base = Path("/output_base")

        mock_read_text.return_value = (
            '{"root_module": "__WORKSPACE__/src/lib.rs"}'
        )

        expected_spec = {"root_module": "/fuchsia_dir/src/lib.rs"}

        result = bazel_rust_analyzer_utils.load_crate_spec_from_json(
            Path("_unused_dummy.json"), mock_paths
        )
        self.assertEqual(result, expected_spec)
        mock_read_text.assert_called_once()

    def test_consolidate_crate_lib_then_test_specs(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=["ID-lib_dep.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=CrateSpecBuild(
                    label="//:mylib",
                    build_file="BUILD.bazel",
                ),
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-extra_test_dep.rs",
                display_name="extra_test_dep",
                edition="2018",
                root_module="extra_test_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-lib_deps.rs",
                display_name="lib_dep",
                edition="2018",
                root_module="lib_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib_test",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=["ID-extra_test_dep.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=True,
                build=None,
            ),
        ]

        expected_crate_specs = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=["ID-lib_dep.rs", "ID-extra_test_dep.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=CrateSpecBuild(
                    label="//:mylib",
                    build_file="BUILD.bazel",
                ),
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-extra_test_dep.rs",
                display_name="extra_test_dep",
                edition="2018",
                root_module="extra_test_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-lib_deps.rs",
                display_name="lib_dep",
                edition="2018",
                root_module="lib_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
        ]

        self.maxDiff = None
        self.assertListEqual(
            bazel_rust_analyzer_utils.consolidate_crate_specs(crate_specs),
            expected_crate_specs,
        )

    def test_consolidate_crate_test_then_lib_specs(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib_test",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=["ID-extra_test_dep.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=True,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=["ID-lib_dep.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=CrateSpecBuild(
                    label="//:mylib",
                    build_file="BUILD.bazel",
                ),
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-extra_test_dep.rs",
                display_name="extra_test_dep",
                edition="2018",
                root_module="extra_test_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-lib_deps.rs",
                display_name="lib_dep",
                edition="2018",
                root_module="lib_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
        ]

        expected_crate_specs = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=["ID-extra_test_dep.rs", "ID-lib_dep.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-extra_test_dep.rs",
                display_name="extra_test_dep",
                edition="2018",
                root_module="extra_test_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-lib_deps.rs",
                display_name="lib_dep",
                edition="2018",
                root_module="lib_dep.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
        ]

        self.maxDiff = None
        self.assertListEqual(
            bazel_rust_analyzer_utils.consolidate_crate_specs(crate_specs),
            expected_crate_specs,
        )

    def test_consolidate_crate_lib_test_main_specs(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib_test",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=True,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib_main",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib2.rs",
                display_name="mylib2",
                edition="2018",
                root_module="mylib2.rs",
                is_workspace_member=True,
                deps=["ID-mylib.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
        ]

        expected_crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib2.rs",
                display_name="mylib2",
                edition="2018",
                root_module="mylib2.rs",
                is_workspace_member=True,
                deps=["ID-mylib.rs"],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=False,
                build=None,
            ),
        ]

        self.maxDiff = None
        for input_crate_specs in itertools.permutations(crate_specs):
            sorted_crate_specs = sorted(
                bazel_rust_analyzer_utils.consolidate_crate_specs(
                    input_crate_specs
                ),
                key=lambda s: s["display_name"],
            )
            self.assertListEqual(sorted_crate_specs, expected_crate_specs)

    def test_consolidate_crate_proc_macro_prefer_exec(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-myproc_macro.rs",
                display_name="myproc_macro",
                edition="2018",
                root_module="myproc_macro.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path="bazel-out/k8-opt-exec-F005BA/bin/myproc_macro/libmyproc_macro-12345.so",
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="proc-macro",
                is_test=False,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-myproc_macro.rs",
                display_name="myproc_macro",
                edition="2018",
                root_module="myproc_macro.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path="bazel-out/k8-fastbuild/bin/myproc_macro/libmyproc_macro-12345.so",
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="proc-macro",
                is_test=False,
                build=None,
            ),
        ]

        for perm in itertools.permutations(crate_specs):
            input_crate_specs = (
                bazel_rust_analyzer_utils.consolidate_crate_specs(perm)
            )

            self.assertListEqual(
                input_crate_specs,
                [
                    CrateSpec(
                        aliases={},
                        crate_id="ID-myproc_macro.rs",
                        display_name="myproc_macro",
                        edition="2018",
                        root_module="myproc_macro.rs",
                        is_workspace_member=True,
                        deps=[],
                        proc_macro_dylib_path="bazel-out/k8-opt-exec-F005BA/bin/myproc_macro/libmyproc_macro-12345.so",
                        source=None,
                        cfg=["test", "debug_assertions"],
                        env={},
                        target="x86_64-uknown-linux-gnu",
                        crate_type="proc-macro",
                        is_test=False,
                        build=None,
                    ),
                ],
            )

    def test_consolidate_create_spec_with_aliases(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=True,
                build=None,
            ),
            CrateSpec(
                aliases={"ID-mylib_dep.rs": "aliased_name"},
                crate_id="ID-mylib.rs",
                display_name="mylib_test",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=True,
                build=None,
            ),
        ]

        for perm in itertools.permutations(crate_specs):
            input_crate_specs = (
                bazel_rust_analyzer_utils.consolidate_crate_specs(perm)
            )

            self.assertListEqual(
                input_crate_specs,
                [
                    CrateSpec(
                        aliases={"ID-mylib_dep.rs": "aliased_name"},
                        crate_id="ID-mylib.rs",
                        display_name="mylib",
                        edition="2018",
                        root_module="mylib.rs",
                        is_workspace_member=True,
                        deps=[],
                        proc_macro_dylib_path=None,
                        source=None,
                        cfg=["test", "debug_assertions"],
                        env={},
                        target="x86_64-uknown-linux-gnu",
                        crate_type="rlib",
                        is_test=True,
                        build=None,
                    ),
                ],
            )

    def test_consolidate_crate_spec_with_sources(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=None,
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=True,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib_test",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=CrateSpecSource(
                    exclude_dirs=["exclude"],
                    include_dirs=["include"],
                ),
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=True,
                build=None,
            ),
        ]

        for perm in itertools.permutations(crate_specs):
            input_crate_specs = (
                bazel_rust_analyzer_utils.consolidate_crate_specs(perm)
            )

            self.assertListEqual(
                input_crate_specs,
                [
                    CrateSpec(
                        aliases={},
                        crate_id="ID-mylib.rs",
                        display_name="mylib",
                        edition="2018",
                        root_module="mylib.rs",
                        is_workspace_member=True,
                        deps=[],
                        proc_macro_dylib_path=None,
                        source=CrateSpecSource(
                            exclude_dirs=["exclude"],
                            include_dirs=["include"],
                        ),
                        cfg=["test", "debug_assertions"],
                        env={},
                        target="x86_64-uknown-linux-gnu",
                        crate_type="rlib",
                        is_test=True,
                        build=None,
                    ),
                ],
            )

    def test_consolidate_crate_spec_with_duplicate_sources(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=CrateSpecSource(
                    exclude_dirs=["exclude"],
                    include_dirs=["include"],
                ),
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="rlib",
                is_test=True,
                build=None,
            ),
            CrateSpec(
                aliases={},
                crate_id="ID-mylib.rs",
                display_name="mylib_test",
                edition="2018",
                root_module="mylib.rs",
                is_workspace_member=True,
                deps=[],
                proc_macro_dylib_path=None,
                source=CrateSpecSource(
                    exclude_dirs=["exclude"],
                    include_dirs=["include"],
                ),
                cfg=["test", "debug_assertions"],
                env={},
                target="x86_64-uknown-linux-gnu",
                crate_type="bin",
                is_test=True,
                build=None,
            ),
        ]

        for perm in itertools.permutations(crate_specs):
            input_crate_specs = (
                bazel_rust_analyzer_utils.consolidate_crate_specs(perm)
            )

            self.assertListEqual(
                input_crate_specs,
                [
                    CrateSpec(
                        aliases={},
                        crate_id="ID-mylib.rs",
                        display_name="mylib",
                        edition="2018",
                        root_module="mylib.rs",
                        is_workspace_member=True,
                        deps=[],
                        proc_macro_dylib_path=None,
                        source=CrateSpecSource(
                            exclude_dirs=["exclude"],
                            include_dirs=["include"],
                        ),
                        cfg=["test", "debug_assertions"],
                        env={},
                        target="x86_64-uknown-linux-gnu",
                        crate_type="rlib",
                        is_test=True,
                        build=None,
                    ),
                ],
            )

    def test_convert_crate_specs_to_rust_project_crates(self) -> None:
        crate_specs: list[CrateSpec] = [
            CrateSpec(
                crate_id="lib_a",
                display_name="lib_a",
                edition="2021",
                root_module="a/src/lib.rs",
                is_workspace_member=True,
                deps=["lib_b"],
                cfg=["test"],
                env={},
                target="x86_64-unknown-linux-gnu",
                crate_type="rlib",
            ),
            CrateSpec(
                crate_id="lib_b",
                display_name="lib_b",
                edition="2021",
                root_module="b/src/lib.rs",
                is_workspace_member=True,
                deps=[],
                cfg=[],
                env={},
                target="x86_64-unknown-linux-gnu",
                crate_type="rlib",
                source=CrateSpecSource(
                    include_dirs=["b/src"],
                    exclude_dirs=[],
                ),
                build=CrateSpecBuild(
                    label="//b:b",
                    build_file="b/BUILD.bazel",
                ),
            ),
            CrateSpec(
                crate_id="bin_c",
                display_name="bin_c",
                edition="2021",
                root_module="c/src/main.rs",
                is_workspace_member=True,
                deps=["lib_a"],
                cfg=[],
                env={},
                target="x86_64-unknown-linux-gnu",
                crate_type="bin",
                is_test=False,
            ),
        ]

        expected_crates = [
            {
                "crate_id": 0,
                "display_name": "bin_c",
                "root_module": "c/src/main.rs",
                "edition": "2021",
                "deps": [{"crate": 1, "name": "lib_a"}],
                "is_workspace_member": True,
                "cfg": [],
                "target": "x86_64-unknown-linux-gnu",
                "env": {},
                "is_proc_macro": False,
                "proc_macro_dylib_path": None,
                "build": None,
            },
            {
                "crate_id": 1,
                "display_name": "lib_a",
                "root_module": "a/src/lib.rs",
                "edition": "2021",
                "deps": [{"crate": 2, "name": "lib_b"}],
                "is_workspace_member": True,
                "cfg": ["test"],
                "target": "x86_64-unknown-linux-gnu",
                "env": {},
                "is_proc_macro": False,
                "proc_macro_dylib_path": None,
                "build": None,
            },
            {
                "crate_id": 2,
                "display_name": "lib_b",
                "root_module": "b/src/lib.rs",
                "edition": "2021",
                "deps": [],
                "is_workspace_member": True,
                "source": {"include_dirs": ["b/src"], "exclude_dirs": []},
                "cfg": [],
                "target": "x86_64-unknown-linux-gnu",
                "env": {},
                "is_proc_macro": False,
                "proc_macro_dylib_path": None,
                "build": {
                    "label": "//b:b",
                    "build_file": "b/BUILD.bazel",
                    "target_kind": "lib",
                },
            },
        ]

        result = bazel_rust_analyzer_utils.convert_crate_specs_to_rust_project_crates(
            crate_specs
        )
        self.assertEqual(result, expected_crates)

    def test_merge_rust_project_jsons_deduplication(self) -> None:
        base_json = {
            "crates": [{"crate_id": 0, "root_module": "a.rs", "target": "host"}]
        }
        merge_json = {
            "crates": [
                {"crate_id": 5, "root_module": "a.rs", "target": "host"},
                {
                    "crate_id": 6,
                    "root_module": "b.rs",
                    "target": "host",
                    "deps": [{"crate": 5, "name": "a"}],
                },
            ]
        }
        expected_json = {
            "crates": [
                {
                    "crate_id": 0,
                    "root_module": "a.rs",
                    "target": "host",
                },
                {
                    "crate_id": 7,  # Remapped from 6 + (0 + 1)
                    "root_module": "b.rs",
                    "target": "host",
                    # Dependency 5 (a.rs) in merge_json is deduplicated to base crate 0.
                    "deps": [{"crate": 0, "name": "a"}],
                },
            ]
        }
        result = bazel_rust_analyzer_utils.merge_rust_project_jsons(
            base_json, [merge_json]
        )
        self.assertEqual(result, expected_json)

    def test_merge_rust_project_jsons_multiple_merges(self) -> None:
        base_json = {
            "crates": [{"crate_id": 0, "root_module": "base.rs", "target": "t"}]
        }
        merge1 = {
            "crates": [{"crate_id": 0, "root_module": "m1.rs", "target": "t"}]
        }
        merge2 = {
            "crates": [{"crate_id": 0, "root_module": "m2.rs", "target": "t"}]
        }

        expected_json = {
            "crates": [
                {"crate_id": 0, "root_module": "base.rs", "target": "t"},
                {
                    "crate_id": 1,
                    "root_module": "m1.rs",
                    "target": "t",
                },  # 0 + (0 + 1)
                {
                    "crate_id": 2,
                    "root_module": "m2.rs",
                    "target": "t",
                },  # 0 + (0 + 1 + 0 + 1)
            ]
        }
        result = bazel_rust_analyzer_utils.merge_rust_project_jsons(
            base_json, [merge1, merge2]
        )
        self.assertEqual(result, expected_json)

    def test_merge_rust_project_jsons_empty_base(self) -> None:
        base_json: dict[str, list[str]] = {"crates": []}
        merge_json = {
            "crates": [{"crate_id": 10, "root_module": "a.rs", "target": "t"}]
        }
        # Offset starts at -1 + 1 = 0. New ID = 10 + 0 = 10.
        expected_json = {
            "crates": [
                {
                    "crate_id": 10,
                    "root_module": "a.rs",
                    "target": "t",
                }
            ]
        }
        result = bazel_rust_analyzer_utils.merge_rust_project_jsons(
            base_json, [merge_json]
        )
        self.assertEqual(result, expected_json)

    def test_merge_rust_project_jsons_broken_dependency(self) -> None:
        base_json = {
            "crates": [{"crate_id": 0, "root_module": "a.rs", "target": "t"}]
        }
        merge_json = {
            "crates": [
                {
                    "crate_id": 0,
                    "root_module": "b.rs",
                    "target": "t",
                    "deps": [{"crate": 999, "name": "missing"}],
                }
            ]
        }
        # Broken dependency should be dropped to avoid invalid crate references.
        expected_json = {
            "crates": [
                {
                    "crate_id": 0,
                    "root_module": "a.rs",
                    "target": "t",
                },
                {
                    "crate_id": 1,
                    "root_module": "b.rs",
                    "target": "t",
                    "deps": [{"crate": 1000, "name": "missing"}],
                },
            ]
        }
        result = bazel_rust_analyzer_utils.merge_rust_project_jsons(
            base_json, [merge_json]
        )
        self.assertEqual(result, expected_json)

    def test_find_crate_for_file(self) -> None:
        crates: list[Crate] = [
            {
                "crate_id": 0,
                "root_module": "/abs/crate_a/src/lib.rs",
                "source": {
                    "include_dirs": ["/abs/crate_a/src"],
                    "exclude_dirs": ["/abs/crate_a/src/exclude"],
                },
            },
            {
                "crate_id": 1,
                "root_module": "/abs/crate_b/src/main.rs",
                "source": {
                    "include_dirs": ["/abs/crate_b/src"],
                    "exclude_dirs": [],
                },
            },
            {
                "crate_id": 2,
                "root_module": "/abs/crate_c/src/lib.rs",
                "source": {
                    "include_dirs": ["/abs/crate_c/src"],
                    "exclude_dirs": [],
                },
            },
            {
                "crate_id": 3,
                "root_module": "/abs/crate_c/src/main.rs",
                "source": {
                    "include_dirs": ["/abs/crate_c/src"],
                    "exclude_dirs": [],
                },
            },
        ]

        test_cases = [
            ("/abs/crate_a/src/lib.rs", {0}),
            ("/abs/crate_a/src/other.rs", {0}),
            ("/abs/crate_a/src/exclude/skipped.rs", set()),
            ("/abs/crate_b/src/main.rs", {1}),
            ("/abs/crate_b/src/mod/foo.rs", {1}),
            ("/abs/unknown/file.rs", set()),
            ("/abs/crate_c/src/foo.rs", {2, 3}),
        ]

        for file_path, expected_crate_ids in test_cases:
            with self.subTest(file_path=file_path):
                crate_result = bazel_rust_analyzer_utils.find_crates_for_file(
                    Path(file_path), crates
                )
                result = {crate["crate_id"] for crate in crate_result}
                self.assertEqual(result, expected_crate_ids)

    def test_get_crates_and_dependencies(self) -> None:
        crates: list[Crate] = [
            {"crate_id": 0, "root_module": "crate0", "deps": []},
            {
                "crate_id": 1,
                "root_module": "crate1",
                "deps": [{"crate": 0, "name": "crate0"}],
            },
            {
                "crate_id": 2,
                "root_module": "crate2",
                "deps": [{"crate": 1, "name": "crate1"}],
            },
            {
                "crate_id": 3,
                "root_module": "crate3",
                "deps": [
                    {"crate": 1, "name": "crate1"},
                    {"crate": 0, "name": "crate0"},
                ],
            },
            {
                "crate_id": 4,
                "root_module": "crate4",
                "deps": [],
            },
        ]

        # No dependencies
        self.assertEqual(
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [crates[0]], crates
            ),
            [crates[0]],
        )

        # Single dependency
        # Result: [crates[1], crates[0]]
        # Mapping: {1: 0, 0: 1}
        expected_crate1 = crates[1].copy()
        expected_crate1["crate_id"] = 0
        expected_crate1["deps"] = [{"crate": 1, "name": "crate0"}]

        expected_crate0 = crates[0].copy()
        expected_crate0["crate_id"] = 1

        self.assertEqual(
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [crates[1]], crates
            ),
            [expected_crate1, expected_crate0],
        )

        # Transitive dependency
        # Result: [crates[2], crates[1], crates[0]]
        # Mapping: {2: 0, 1: 1, 0: 2}
        expected_crate2 = crates[2].copy()
        expected_crate2["crate_id"] = 0
        expected_crate2["deps"] = [{"crate": 1, "name": "crate1"}]

        expected_crate1 = crates[1].copy()
        expected_crate1["crate_id"] = 1
        expected_crate1["deps"] = [{"crate": 2, "name": "crate0"}]

        expected_crate0 = crates[0].copy()
        expected_crate0["crate_id"] = 2

        self.assertEqual(
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [crates[2]], crates
            ),
            [expected_crate2, expected_crate1, expected_crate0],
        )

        # Diamond dependency (crate3 -> crate1 -> crate0, crate3 -> crate0)
        # Result: [crates[3], crates[1], crates[0]]
        # Mapping: {3: 0, 1: 1, 0: 2}
        expected_crate3 = crates[3].copy()
        expected_crate3["crate_id"] = 0
        expected_crate3["deps"] = [
            {"crate": 1, "name": "crate1"},
            {"crate": 2, "name": "crate0"},
        ]

        expected_crate1 = crates[1].copy()
        expected_crate1["crate_id"] = 1
        expected_crate1["deps"] = [{"crate": 2, "name": "crate0"}]

        expected_crate0 = crates[0].copy()
        expected_crate0["crate_id"] = 2

        self.assertEqual(
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [crates[3]], crates
            ),
            [expected_crate3, expected_crate1, expected_crate0],
        )

        # Multiple interest.
        # Result: [crates[4], crates[1], crates[0]]
        # Mapping: {4: 0, 1: 1, 0: 2}
        expected_crate4 = crates[4].copy()
        expected_crate4["crate_id"] = 0
        expected_crate4["deps"] = []

        expected_crate1 = crates[1].copy()
        expected_crate1["crate_id"] = 1
        expected_crate1["deps"] = [{"crate": 2, "name": "crate0"}]

        expected_crate0 = crates[0].copy()
        expected_crate0["crate_id"] = 2

        self.assertEqual(
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [crates[4], crates[1]], crates
            ),
            [expected_crate4, expected_crate1, expected_crate0],
        )

    def test_get_crates_and_dependencies_value_error(self) -> None:
        crates: list[Crate] = [
            {"crate_id": 0, "root_module": "crate0", "deps": []},
            {
                "crate_id": 1,
                "root_module": "crate1",
                "deps": [{"crate": 0, "name": "crate0"}],
            },
        ]

        with self.assertRaisesRegex(
            ValueError, "Crate 2 not found in crate list"
        ):
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [{"crate_id": 2, "root_module": "crate2"}], crates
            )

    def test_get_crates_and_dependencies_missing_dependency_error(self) -> None:
        crates: list[Crate] = [
            {
                "crate_id": 1,
                "root_module": "crate1",
                "deps": [{"crate": 999, "name": "missing"}],
            },
        ]

        with self.assertRaisesRegex(
            ValueError, "Dependency crate 999 not found in crate list"
        ):
            bazel_rust_analyzer_utils.get_crates_and_dependencies(
                [crates[0]], crates
            )


if __name__ == "__main__":
    unittest.main()
