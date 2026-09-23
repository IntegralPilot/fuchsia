# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for bazel_build_args.py."""

import io
import os
import sys
import tempfile
import unittest
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import bazel_build_args
import build_utils
from bazel_build_args import (
    ParsedAction,
    ResolvedBuildArgsFlags,
    ResolvedBuildArgsMap,
    ResponseFileMap,
    ResponseFileTarget,
)


class PathToLabelTest(unittest.TestCase):
    def test_valid_path_parsing(self) -> None:
        TEST_CASES = [
            (
                "bazel-out/k8-fastbuild/bin/build/bazel/examples/hello_world/main.cpp_compile.build_flags",
                ResponseFileTarget(
                    "@@//build/bazel/examples/hello_world:main.final_build_flags",
                    "cpp_compile",
                ),
            ),
            (
                "bazel-out/fuchsia_platform_arm64-fastbuild/bin/src/my_pkg/tool.cpp_link.build_flags",
                ResponseFileTarget(
                    "@@//src/my_pkg:tool.final_build_flags", "cpp_link"
                ),
            ),
            (
                "execroot/_main/bazel-out/linux_x64-opt/bin/external/rules_rust/tool.rust_compile.build_flags",
                ResponseFileTarget(
                    "@@rules_rust//:tool.final_build_flags", "rust_compile"
                ),
            ),
            (
                "${pwd}/bazel-out/k8-fastbuild/bin/build/target.rust_compile.build_flags",
                ResponseFileTarget(
                    "@@//build:target.final_build_flags", "rust_compile"
                ),
            ),
            (
                "${pwd}/bazel-out/k8-fastbuild/bin/build/target.rustc_env_file.build_flags",
                ResponseFileTarget(
                    "@@//build:target.final_build_flags", "rust_compile"
                ),
            ),
        ]
        for path, expected in TEST_CASES:
            result = ResponseFileTarget.from_execroot_path(path)
            self.assertEqual(result, expected, msg=f"For path [{path}]")

    def test_invalid_path_returns_none(self) -> None:
        TEST_CASES = [
            "src/my_pkg/tool.cpp_compile.build_flags",  # no bazel-out/../bin/
            "bazel-out/k8-fastbuild/bin/build/target.unknown_suffix",
            "bazel-out/bin/too_short.cpp_compile.build_flags",  # config dir missing
            "",
        ]
        for path in TEST_CASES:
            self.assertIsNone(
                ResponseFileTarget.from_execroot_path(path),
                msg=f"Expected None for [{path}]",
            )


class ParseAqueryOutputTest(unittest.TestCase):
    def test_parse_aquery_no_filter(self) -> None:
        aquery_output = """
{
  "targets": [
    { "id": 1, "label": "//src:main" },
    { "id": 2, "label": "//src:lib" }
  ],
  "configuration": [
    { "id": 1, "mnemonic": "k8-fastbuild" }
  ],
  "actions": [
    {
      "targetId": 1,
      "mnemonic": "CppCompile",
      "configurationId": 1,
      "arguments": ["exec", "clang", "-c", "src/main.cc", "@bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags", "-o", "main.o"],
      "environmentVariables": [
        { "key": "PATH", "value": "/bin" }
      ]
    },
    {
      "targetId": 1,
      "mnemonic": "CppLink",
      "configurationId": 1,
      "arguments": ["exec", "clang", "main.o", "-o", "bin", "@bazel-out/k8-fastbuild/bin/main.cpp_link.build_flags"]
    }
  ]
}
"""
        result = bazel_build_args.parse_aquery_output(aquery_output)

        self.assertEqual(len(result.actions), 2)
        self.assertEqual(result.actions[0].mnemonic, "CppCompile")
        self.assertEqual(result.actions[0].target, "//src:main")
        self.assertEqual(result.actions[0].env_vars, {"PATH": "/bin"})
        self.assertListEqual(
            result.actions[0].args,
            [
                "exec",
                "clang",
                "-c",
                "src/main.cc",
                "@bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags",
                "-o",
                "main.o",
            ],
        )

        self.assertEqual(result.actions[1].mnemonic, "CppLink")
        self.assertListEqual(
            result.actions[1].args,
            [
                "exec",
                "clang",
                "main.o",
                "-o",
                "bin",
                "@bazel-out/k8-fastbuild/bin/main.cpp_link.build_flags",
            ],
        )

        self.assertDictEqual(
            result.response_files_map,
            {
                "bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags": (
                    ResponseFileTarget(
                        "@@//src:main.final_build_flags", "cpp_compile"
                    )
                ),
                "bazel-out/k8-fastbuild/bin/main.cpp_link.build_flags": (
                    ResponseFileTarget(
                        "@@//:main.final_build_flags", "cpp_link"
                    )
                ),
            },
        )

    def test_parse_aquery_with_mnemonic_filter(self) -> None:
        aquery_output = """
{
  "targets": [
    { "id": 1, "label": "//src:main" },
    { "id": 2, "label": "//src:lib" }
  ],
  "configuration": [
    { "id": 1, "mnemonic": "k8-fastbuild" }
  ],
  "actions": [
    {
      "targetId": 1,
      "mnemonic": "CppCompile",
      "configurationId": 1,
      "arguments": ["exec", "clang", "-c", "src/main.cc", "@bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags"]
    },
    {
      "targetId": 2,
      "mnemonic": "CppLink",
      "configurationId": 1,
      "arguments": ["exec", "clang", "main.o", "-o", "bin", "@bazel-out/k8-fastbuild/bin/main.cpp_link.build_flags"]
    }
  ]
}
"""
        result = bazel_build_args.parse_aquery_output(
            aquery_output,
            filter_mnemonics=["CppCompile"],
        )

        self.assertEqual(len(result.actions), 1)
        self.assertEqual(result.actions[0].mnemonic, "CppCompile")
        self.assertDictEqual(
            result.response_files_map,
            {
                "bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags": (
                    ResponseFileTarget(
                        "@@//src:main.final_build_flags", "cpp_compile"
                    )
                ),
            },
        )

    def test_parse_aquery_malformed_json_returns_empty(self) -> None:
        suppressed_stderr = io.StringIO()
        orig_stderr = sys.stderr
        sys.stderr = suppressed_stderr
        try:
            result = bazel_build_args.parse_aquery_output("malformed { json }")
        finally:
            sys.stderr = orig_stderr

        self.assertEqual(len(result.actions), 0)
        self.assertEqual(len(result.response_files_map), 0)


class ExpandArgsFromDiskTest(unittest.TestCase):
    def test_recursive_disk_expansion(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            execroot = Path(tmp_dir)

            # Create a standard .params file
            params_file = execroot / "target.params"
            params_file.write_text("-DCOMPILE_FLAG\n@nested.flags\n")

            # Create a nested custom flags file
            nested_file = execroot / "nested.flags"
            nested_file.write_text("-DNESTED_FLAG\n--env-file\nenv.vars\n")

            # Create an env file
            env_file = execroot / "env.vars"
            env_file.write_text("KEY=VAL\n")

            raw_args = [
                "exec",
                "clang",
                "@target.params",
                "-o",
                "output",
            ]

            result = bazel_build_args.expand_args_from_disk(
                raw_args, {}, str(execroot)
            )

            self.assertListEqual(
                result.expanded_args,
                [
                    "exec",
                    "clang",
                    "-DCOMPILE_FLAG",
                    "-DNESTED_FLAG",
                    "-o",
                    "output",
                ],
            )
            self.assertListEqual(result.env_vars, ["KEY=VAL"])
            self.assertEqual(len(result.warnings), 0)

    def test_cycle_detection(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            execroot = Path(tmp_dir)

            # Create a cycling params file
            params_file = execroot / "cycle.params"
            params_file.write_text("@cycle.params\n")

            raw_args = ["@cycle.params"]

            result = bazel_build_args.expand_args_from_disk(
                raw_args, {}, str(execroot)
            )

            self.assertListEqual(result.expanded_args, ["@cycle.params"])
            self.assertListEqual(result.env_vars, [])
            self.assertEqual(len(result.warnings), 1)
            self.assertEqual(
                result.warnings[0],
                "Cycle detected in response file @cycle.params: cycle.params -> cycle.params",
            )

    def test_missing_file_reports_warning(self) -> None:
        result = bazel_build_args.expand_args_from_disk(
            ["@missing.params"], {}, "/non_existent_root"
        )

        self.assertListEqual(result.expanded_args, ["@missing.params"])
        self.assertListEqual(result.env_vars, [])
        self.assertEqual(len(result.warnings), 1)
        self.assertIn("was not found on disk", result.warnings[0])

    def test_safe_path_normalization_prevents_false_positives(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            execroot = Path(tmp_dir)

            pwd_sub_dir = execroot / "mypwd"
            pwd_sub_dir.mkdir()

            nested_file = pwd_sub_dir / "target.params"
            nested_file.write_text("-DCOMPILE_FLAG\n")

            raw_args = ["@mypwd/target.params"]

            result = bazel_build_args.expand_args_from_disk(
                raw_args, {}, str(execroot)
            )

            self.assertListEqual(result.expanded_args, ["-DCOMPILE_FLAG"])
            self.assertListEqual(result.env_vars, [])
            self.assertEqual(len(result.warnings), 0)

    def test_normalize_path_strips_env_pwd_but_keeps_folder_pwd(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            execroot = Path(tmp_dir)

            # Directory named "pwd" should be kept
            pwd_dir = execroot / "pwd"
            pwd_dir.mkdir()

            file_in_pwd = pwd_dir / "target.params"
            file_in_pwd.write_text("-DPWD_FOLDER_FLAG\n")

            # File in root to be found via stripped ${pwd}/ prefix
            file_in_root = execroot / "target.params"
            file_in_root.write_text("-DROOT_FLAG\n")

            # Test that ${pwd}/ prefix gets stripped and targets file in root
            result_stripped = bazel_build_args.expand_args_from_disk(
                ["@${pwd}/target.params"], {}, str(execroot)
            )
            self.assertListEqual(result_stripped.expanded_args, ["-DROOT_FLAG"])

            # Test that pwd/ prefix is NOT stripped and targets file in pwd/ directory
            result_not_stripped = bazel_build_args.expand_args_from_disk(
                ["@pwd/target.params"], {}, str(execroot)
            )
            self.assertListEqual(
                result_not_stripped.expanded_args, ["-DPWD_FOLDER_FLAG"]
            )


class ExpandArgsStaticTest(unittest.TestCase):
    def test_static_expansion(self) -> None:
        raw_args = [
            "exec",
            "clang",
            "@bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags",
            "@bazel-out/k8-fastbuild/bin/target.params",
            "--env-file",
            "bazel-out/k8-fastbuild/bin/env.rustc_env_file.build_flags",
        ]

        response_files_map = ResponseFileMap(
            {
                "bazel-out/k8-fastbuild/bin/src/main.cpp_compile.build_flags": (
                    ResponseFileTarget(
                        "@@//src:main.final_build_flags", "cpp_compile"
                    )
                ),
                "bazel-out/k8-fastbuild/bin/env.rustc_env_file.build_flags": (
                    ResponseFileTarget(
                        "@@//src:main.final_build_flags", "rust_compile"
                    )
                ),
            }
        )

        build_args_map = ResolvedBuildArgsMap(
            {
                "@@//src:main.final_build_flags": ResolvedBuildArgsFlags(
                    label="@@//src:main.final_build_flags",
                    cflags=["-DCOMPILE_FLAG"],
                    cflags_c=[],
                    cflags_cc=["-DCC_FLAG"],
                    defines=[],
                    include_dirs=[],
                    ldflags=[],
                    lib_dirs=[],
                    rustflags=[],
                    rustenv=["KEY=VAL"],
                )
            }
        )

        result = bazel_build_args.expand_args_with_build_args_map(
            raw_args, {}, response_files_map, build_args_map
        )

        self.assertListEqual(
            result.expanded_args,
            [
                "exec",
                "clang",
                "-DCOMPILE_FLAG",
                "-DCC_FLAG",
                "@bazel-out/k8-fastbuild/bin/target.params",
            ],
        )
        self.assertListEqual(result.env_vars, ["KEY=VAL"])

        # Verify that the standard .params file registers a warning and is kept verbatim
        self.assertEqual(len(result.warnings), 1)
        self.assertIn("cannot be expanded with queries", result.warnings[0])


class GetBazelExpandedActionsTest(unittest.TestCase):
    def test_get_bazel_expanded_actions_static(self) -> None:
        # Setup mocks
        mock_launcher = build_utils.MockBazelLauncher()
        mock_launcher.push_expected_outputs(['{"some": "json"}'])

        # Mock aquery result
        actions = [
            ParsedAction(
                name="action1",
                target="//src:main",
                config="config",
                mnemonic="CppCompile",
                args=[
                    "exec",
                    "clang",
                    "@bazel-out/bin/main.cpp_compile.build_flags",
                ],
                env_vars={"FOO": "foo"},
            )
        ]
        response_files_map = ResponseFileMap(
            {
                "bazel-out/bin/main.cpp_compile.build_flags": ResponseFileTarget(
                    "@@//src:main.final_build_flags", "cpp_compile"
                )
            }
        )

        # Monkeypatch
        orig_parse_aquery = bazel_build_args.parse_aquery_output
        orig_query_flags = bazel_build_args.query_build_flags_from_bazel

        bazel_build_args.parse_aquery_output = (
            lambda aquery_output, filter_mnemonics=None: (
                bazel_build_args.ParsedAqueryResult(
                    actions=actions,
                    response_files_map=response_files_map,
                    uses_bazel_params_file=False,
                )
            )
        )
        bazel_build_args.query_build_flags_from_bazel = lambda final_build_flags_labels, bazel_launcher, config_args: (
            ResolvedBuildArgsMap(
                {
                    "@@//src:main.final_build_flags": ResolvedBuildArgsFlags(
                        label="@@//src:main.final_build_flags",
                        cflags=["-DSTATIC_FLAG"],
                        cflags_c=[],
                        cflags_cc=[],
                        defines=[],
                        include_dirs=[],
                        ldflags=[],
                        lib_dirs=[],
                        rustflags=[],
                        rustenv=[],
                    )
                }
            )
        )

        try:
            expanded = bazel_build_args.get_bazel_expanded_actions(
                bazel_launcher=mock_launcher,
                bazel_execroot="/dummy_root",
                bazel_target="//src:main",
                config_args=["--config=fuchsia"],
                read_response_files=False,
            )

            self.assertEqual(len(expanded), 1)
            self.assertEqual(expanded[0].action, "action1")
            self.assertEqual(expanded[0].target, "//src:main")
            self.assertListEqual(
                expanded[0].args, ["exec", "clang", "-DSTATIC_FLAG"]
            )
            self.assertListEqual(expanded[0].env_vars, ["FOO=foo"])
            self.assertEqual(len(expanded[0].warnings), 0)
        finally:
            # Restore original functions
            bazel_build_args.parse_aquery_output = orig_parse_aquery
            bazel_build_args.query_build_flags_from_bazel = orig_query_flags

    def test_get_bazel_expanded_actions_dynamic(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            execroot = Path(tmp_dir)
            flags_file = execroot / "bazel-out/bin/main.cpp_compile.build_flags"
            flags_file.parent.mkdir(parents=True, exist_ok=True)
            flags_file.write_text("-DDYNAMIC_FLAG\n")

            mock_launcher = build_utils.MockBazelLauncher()
            mock_launcher.push_expected_outputs(["{}"])

            actions = [
                ParsedAction(
                    name="action1",
                    target="//src:main",
                    config="config",
                    mnemonic="CppCompile",
                    args=[
                        "exec",
                        "clang",
                        f"@bazel-out/bin/main.cpp_compile.build_flags",
                    ],
                    env_vars={"FOO": "foo"},
                )
            ]

            # Monkeypatch
            orig_parse_aquery = bazel_build_args.parse_aquery_output
            bazel_build_args.parse_aquery_output = (
                lambda aquery_output, filter_mnemonics=None: (
                    bazel_build_args.ParsedAqueryResult(
                        actions=actions,
                        response_files_map=ResponseFileMap({}),
                        uses_bazel_params_file=False,
                    )
                )
            )

            try:
                expanded = bazel_build_args.get_bazel_expanded_actions(
                    bazel_launcher=mock_launcher,
                    bazel_execroot=str(execroot),
                    bazel_target="//src:main",
                    config_args=[],
                    read_response_files=True,
                )

                self.assertEqual(len(expanded), 1)
                self.assertEqual(expanded[0].action, "action1")
                self.assertListEqual(
                    expanded[0].args, ["exec", "clang", "-DDYNAMIC_FLAG"]
                )
                self.assertListEqual(expanded[0].env_vars, ["FOO=foo"])
            finally:
                # Restore
                bazel_build_args.parse_aquery_output = orig_parse_aquery


if __name__ == "__main__":
    unittest.main()
