#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections.abc
import json
import os
import subprocess
import sys
import tempfile
import typing as T
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
_FUCHSIA_DIR = _SCRIPT_DIR.parent.parent
_BUILD_API_SCRIPT = _SCRIPT_DIR / "client"

# While these values are also defined in ninja_artifacts.py, redefine
# them here to avoid an import statement, as this regression test
# should not depend on implementation details of the client.py
# script.
_NINJA_BUILD_PLAN_DEPS_FILE = "build.ninja.d"
_NINJA_LAST_BUILD_TARGETS_FILE = "last_ninja_build_targets.txt"
_NINJA_LAST_BUILD_SUCCESS_FILE = "last_ninja_build_success.stamp"

CommandResult: T.TypeAlias = subprocess.CompletedProcess[str]

CommandArguments: T.TypeAlias = collections.abc.Sequence[str | Path]


def _write_file(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def _write_json(path: Path, content: T.Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w") as f:
        json.dump(content, f, sort_keys=True)


class ClientTestBase(unittest.TestCase):
    def setUp(self) -> None:
        """Common setup for all test classes."""
        self._temp_dir = tempfile.TemporaryDirectory()
        self._top_dir = Path(self._temp_dir.name)
        (self._top_dir / ".jiri_manifest").write_text("")

        self._build_gn_path = self._top_dir / "BUILD.gn"
        self._build_gn_path.write_text("# EMPTY\n")

        self._build_dir = self._top_dir / "out" / "build_dir"
        self._build_dir.mkdir(parents=True)

        self._build_ninja_d_path = self._build_dir / _NINJA_BUILD_PLAN_DEPS_FILE
        _write_file(
            self._build_ninja_d_path, "build.ninja.stamp: ../../BUILD.gn\n"
        )

        self._bazel_workspace = self._build_dir / "bazel_workspace"
        _write_file(
            self._top_dir / "build/bazel/config/bazel_top_dir",
            "bazel_workspace",
        )

        # A fake host tag used to verify that the command used the --host-tag value
        # properly, instead of picking the real one by mistake.
        self._host_tag = "linux-y64"

        # Compute the real host tag to locate the Ninja binary.
        real_host_arch = os.uname().machine
        real_host_arch = {
            "x86_64": "x64",
            "aarch64": "arm64",
        }.get(real_host_arch, real_host_arch)

        real_host_tag = f"{sys.platform}-{real_host_arch}"

        real_ninja_path = (
            _FUCHSIA_DIR / f"prebuilt/third_party/ninja/{real_host_tag}/ninja"
        )
        assert (
            real_ninja_path.exists()
        ), f"Missing Ninja binary: {real_ninja_path}"

        # Create symlink to real Ninja binary here.
        self._ninja_path = (
            self._top_dir / f"prebuilt/third_party/ninja/{self._host_tag}/ninja"
        )
        self._ninja_path.parent.mkdir(parents=True)
        self._ninja_path.symlink_to(real_ninja_path)

        self._last_build_success_path = (
            self._build_dir / _NINJA_LAST_BUILD_SUCCESS_FILE
        )
        self._last_targets_path = (
            self._build_dir / _NINJA_LAST_BUILD_TARGETS_FILE
        )
        self._build_ninja_path = self._build_dir / "build.ninja"

        # The build_api_client_info file maps each module name to its .json file.
        _write_file(
            self._build_dir / "build_api_client_info",
            "\n".join(
                [
                    "args=args.json",
                    "build_info=build_info.json",
                    "debug_symbols=debug_symbols.json",
                    "tests=tests.json",
                ]
            ),
        )

        # The $BUILD_DIR/args.json file is necessary to extract the
        # target cpu value.
        self._args_json = json.dumps({"target_cpu": "aRm64"})

        # Fake tests.json with a single entry.
        self._tests_json = json.dumps(
            [
                {
                    "environments": [
                        {
                            "dimensions": {
                                "cpu": "y64",
                                "os": "Linux",
                            }
                        }
                    ],
                    "test": {
                        "cpu": "y64",
                        "label": "//some/test:target(//build/toolchain:host_y64)",
                        "name": "host_y64/obj/some/test/target_test.sh",
                        "os": "linux",
                        "path": "host_y64/obj/some/test/target_test.sh",
                        "runtime_deps": "host_y64/gen/some/test/target_test.deps.json",
                    },
                },
            ]
        )

        # Fake build_info.json
        self._build_info_json = json.dumps(
            {
                "configurations": [
                    {
                        "board": "y64",
                        "product": "core",
                    },
                ],
                "version": "",
            },
        )

        _write_file(self._build_dir / "args.json", self._args_json)
        _write_file(self._build_dir / "tests.json", self._tests_json)
        _write_file(self._build_dir / "build_info.json", self._build_info_json)

        # Fake Ninja outputs.
        self._ninja_outputs = {
            "//foo:foo": [
                "obj/foo.stamp",
            ],
            "//bar:bar": [
                "obj/bar.output",
                "obj/bar.stamp",
            ],
            "//src:lib": [
                "obj/src/lib.cc.o",
            ],
            "//src:bin": [
                "obj/src/main.cc.o",
                "obj/src/program",
            ],
            "//tools:hammer(//build/toolchain:host_y64)": [
                "host_y64/exe.unstripped/hammer",
                "host_y64/hammer",
                "host_y64/obj/tools/hammer.cc.o",
            ],
            "//some/test:target(//build/toolchain:host_y64)": [
                "host_y64/obj/some/test/target_test.sh",
                "host_y64/gen/some/test/target_test.deps.json",
            ],
        }
        _write_json(self._build_dir / "ninja_outputs.json", self._ninja_outputs)

        # Fake debug symbols
        self._debug_symbols_json = json.dumps(
            [
                {
                    "cpu": "x64",
                    "debug": "obj/src/foo/lib_shared/libfoo.so.unstripped",
                    "elf_build_id": "00000000000000001",
                    "label": "//src/foo:lib_shared",
                    "os": "fuchsia",
                },
                {
                    "cpu": "x64",
                    "debug": "../../prebuilt/.build-id/aa/bbbbbbbbbbb.debug",
                    "label": "//prebuilt/foo:symbol_file",
                    "os": "fuchsia",
                },
                {
                    "cpu": "x64",
                    "debug": "obj/src/bar/binary.unstripped",
                    "elf_build_id_file": "obj/src/bar/binary.elf_build_id",
                    "label": "//src/bar:binary",
                    "os": "fuchsia",
                },
                {
                    "cpu": "x64",
                    "debug": "obj/src/zoo/binary.unstripped",
                    "label": "//src/zoo:binary",
                    "os": "fuchsia",
                },
            ]
        )
        _write_file(
            self._build_dir / "debug_symbols.json", self._debug_symbols_json
        )
        _write_file(
            self._build_dir / "obj/src/bar/binary.elf_build_id",
            "build_id_for_bar",
        )

    def tearDown(self) -> None:
        """Common cleanup for all test classes."""
        self._temp_dir.cleanup()

    def run_client(self, args: CommandArguments) -> CommandResult:
        """Run a //build/api/client command and return results after capturing output as text.

        This runs the command in the test's build directory to mimic calls from Ninja actions.

        Args:
            args: The command name followed by its optional arguments.
        Returns:
            A CommandResult value.
        """
        return subprocess.run(
            [
                str(a)
                for a in [
                    _BUILD_API_SCRIPT,
                    "--fuchsia-dir",
                    self._top_dir,
                    "--build-dir",
                    self._build_dir,
                    f"--host-tag={self._host_tag}",
                    *args,
                ]
            ],
            cwd=self._build_dir,
            text=True,
            capture_output=True,
        )

    def assert_command_result(
        self,
        ret: CommandResult,
        expected_out: str,
        expected_err: str = "",
        expected_status: int = 0,
        msg: str = "",
    ) -> None:
        """Assert the result of executing a given command with run_client().

        Args:
            raw_ret: A CommandResult value from run_client().
            expected_out: The expected stdout.
            expected_err: The expected stderr, defaults to an empty string.
            expected_status: The expected status code, defaults to 0.
            msg: Optional message printed in case of assertion failure. If
               empty (the default), the command arguments will be used instead
        """
        if not expected_err and ret.stderr:
            print(f"ERROR: {ret.stderr}", file=sys.stderr)
        self.assertEqual(expected_err, ret.stderr, msg=msg)
        self.assertEqual(expected_out, ret.stdout, msg=msg)
        self.assertEqual(expected_status, ret.returncode, msg=msg)

    def assert_output(
        self,
        args: CommandArguments,
        expected_out: str,
        expected_err: str = "",
        expected_status: int = 0,
        msg: str = "",
    ) -> None:
        """Run a command through run_client() then call assert_command_result() on its result.

        Args:
            args: A sequence of command arguments.
            expected_out: The expected stdout.
            expected_err: The expected stderr, defaults to an empty string.
            expected_status: The expected status code, defaults to 0.
            msg: Optional message printed in case of failure.
        """
        return self.assert_command_result(
            self.run_client(args),
            expected_out,
            expected_err,
            expected_status,
            msg="'%s' command" % " ".join(str(a) for a in args)
            + (": " + msg if msg else ""),
        )

    def assert_error(
        self,
        args: CommandArguments,
        expected_err: str,
        msg: str = "",
    ) -> None:
        """Run a command through run_client() and expect it to fail with no output

        This also assumes that the status code is 1.

        Args:
            args: A sequence of command arguments.
            expected_err: The expected stderr.
            msg: Optional message printed in case of failure.
        """
        self.assert_output(args, "", expected_err, expected_status=1, msg=msg)


class ClientTest(ClientTestBase):
    def test_list(self) -> None:
        self.assert_output(["list"], "args\nbuild_info\ndebug_symbols\ntests\n")

    def test_print(self) -> None:
        MODULES = {
            "args": self._args_json + "\n",
            "tests": self._tests_json + "\n",
            "build_info": self._build_info_json + "\n",
        }
        for module, expected in MODULES.items():
            self.assert_output(["print", module], expected)

    def test_print_all(self) -> None:
        expected = {
            "args": {
                "file": "args.json",
                "json": json.loads(self._args_json),
            },
            "build_info": {
                "file": "build_info.json",
                "json": json.loads(self._build_info_json),
            },
            "debug_symbols": {
                "file": "debug_symbols.json",
                "json": json.loads(self._debug_symbols_json),
            },
            "tests": {
                "file": "tests.json",
                "json": json.loads(self._tests_json),
            },
        }
        self.assert_output(["print_all"], json.dumps(expected) + "\n")
        self.assert_output(
            ["print_all", "--pretty"], json.dumps(expected, indent=2) + "\n"
        )

    def test_print_debug_symbols(self) -> None:
        self.maxDiff = None
        expected = [
            {
                "cpu": "x64",
                "debug": "obj/src/foo/lib_shared/libfoo.so.unstripped",
                "elf_build_id": "00000000000000001",
                "label": "//src/foo:lib_shared",
                "os": "fuchsia",
            },
            {
                "cpu": "x64",
                "debug": "../../prebuilt/.build-id/aa/bbbbbbbbbbb.debug",
                "label": "//prebuilt/foo:symbol_file",
                "os": "fuchsia",
            },
            {
                "cpu": "x64",
                "debug": "obj/src/bar/binary.unstripped",
                "elf_build_id_file": "obj/src/bar/binary.elf_build_id",
                "label": "//src/bar:binary",
                "os": "fuchsia",
            },
            {
                "cpu": "x64",
                "debug": "obj/src/zoo/binary.unstripped",
                "label": "//src/zoo:binary",
                "os": "fuchsia",
            },
        ]
        self.assert_output(["print_debug_symbols"], json.dumps(expected) + "\n")
        self.assert_output(
            ["print_debug_symbols", "--pretty"],
            json.dumps(expected, indent=2) + "\n",
        )

    def test_print_debug_symbols_with_build_id_resolution(self) -> None:
        self.maxDiff = None
        expected = [
            {
                "cpu": "x64",
                "debug": "obj/src/foo/lib_shared/libfoo.so.unstripped",
                "elf_build_id": "00000000000000001",
                "label": "//src/foo:lib_shared",
                "os": "fuchsia",
            },
            {
                "cpu": "x64",
                "debug": "../../prebuilt/.build-id/aa/bbbbbbbbbbb.debug",
                "elf_build_id": "aabbbbbbbbbbb",
                "label": "//prebuilt/foo:symbol_file",
                "os": "fuchsia",
            },
            {
                "cpu": "x64",
                "debug": "obj/src/bar/binary.unstripped",
                "elf_build_id": "build_id_for_bar",
                "elf_build_id_file": "obj/src/bar/binary.elf_build_id",
                "label": "//src/bar:binary",
                "os": "fuchsia",
            },
            # NOTE: Because of the --test-mode flag used below, the build-id value for the
            # file obj/src/zoo/binary.unstripped is just its file name. This avoids creating
            # a fake ELF file in the test build directory.
            {
                "cpu": "x64",
                "debug": "obj/src/zoo/binary.unstripped",
                "elf_build_id": "binary.unstripped",
                "label": "//src/zoo:binary",
                "os": "fuchsia",
            },
        ]
        self.assert_output(
            ["print_debug_symbols", "--resolve-build-ids", "--test-mode"],
            json.dumps(expected) + "\n",
        )
        self.assert_output(
            [
                "print_debug_symbols",
                "--resolve-build-ids",
                "--test-mode",
                "--pretty",
            ],
            json.dumps(expected, indent=2) + "\n",
        )

    def test_ninja_path_to_gn_label(self) -> None:
        # Test each Ninja path individually.
        for label, paths in self._ninja_outputs.items():
            for path in paths:
                self.assert_output(
                    ["ninja_path_to_gn_label", path], f"{label}\n"
                )

        # Test each set of Ninja output paths per label.
        for label, paths in self._ninja_outputs.items():
            self.assert_output(["ninja_path_to_gn_label"] + paths, f"{label}\n")

        # Test a single invocation with all Ninja paths, which must return the set of all labels,
        # deduplicated.
        all_paths = set()
        for paths in self._ninja_outputs.values():
            all_paths.update(paths)
        all_labels = sorted(set(self._ninja_outputs.keys()))
        expected = "\n".join(all_labels) + "\n"
        self.assert_output(
            ["ninja_path_to_gn_label"] + sorted(all_paths), expected
        )

        # Test unknown Ninja path
        self.assert_error(
            ["ninja_path_to_gn_label", "obj/unknown/path"],
            "ERROR: Unknown Ninja target path: obj/unknown/path\n",
        )

    def test_gn_labels_to_ninja_paths(self) -> None:
        # Test each label individually.
        for label, paths in self._ninja_outputs.items():
            expected = "\n".join(sorted(paths)) + "\n"
            self.assert_output(["gn_label_to_ninja_paths", label], expected)

        # Test all labels at the same time
        all_paths = set()
        for paths in self._ninja_outputs.values():
            all_paths.update(paths)
        expected = "\n".join(sorted(all_paths)) + "\n"
        self.assert_output(
            ["gn_label_to_ninja_paths"] + list(self._ninja_outputs.keys()),
            expected,
        )

        # Test unknown GN label
        self.assert_error(
            ["gn_label_to_ninja_paths", "//unknown:label"],
            "ERROR: Unknown GN label (not in the configured graph): //unknown:label\n",
        )

        # Test unknown GN label
        self.assert_output(
            [
                "gn_label_to_ninja_paths",
                "--allow-unknown",
                "unknown_path",
                "unknown:label",
            ],
            "unknown:label\nunknown_path\n",
        )

        # Test that labels are properly qualified before looking into the database.
        self.assert_output(
            [
                "gn_label_to_ninja_paths",
                "//bar(//build/toolchain/fuchsia:aRm64)",
            ],
            "obj/bar.output\nobj/bar.stamp\n",
        )

        # Test that --allow_unknown does not pass unknown GN labels or absolute file paths.
        self.assert_error(
            [
                "gn_label_to_ninja_paths",
                "--allow-unknown",
                "//unknown:label",
            ],
            "ERROR: Unknown GN label (not in the configured graph): //unknown:label\n",
        )

        self.assert_error(
            [
                "gn_label_to_ninja_paths",
                "--allow-unknown",
                "/unknown/path",
            ],
            "ERROR: Absolute path is not a valid GN label or Ninja path: /unknown/path\n",
        )

    def test_gn_labels_to_ninja_paths_bazel_hints(self) -> None:
        # Test Bazel host target auto-mapping from bazel_root_targets.json
        self._ninja_outputs[
            "//build/bazel/host:bazel_root_host_tools.bar(//build/toolchain:host_y64)"
        ] = ["host_y64/bar"]
        _write_json(self._build_dir / "ninja_outputs.json", self._ninja_outputs)
        _write_json(
            self._build_dir / "bazel_root_targets.json",
            [
                {
                    "bazel_label": "//tools/foo:bar",
                    "host_bin_label": "//build/bazel/host:bazel_root_host_tools.bar(//build/toolchain:host_y64)",
                    "gn_subtarget_label": "//build/bazel/host:bazel_root_targets.bar(//build/toolchain/fuchsia:aRm64)",
                }
            ],
        )
        self.assert_output(
            ["gn_label_to_ninja_paths", "//tools/foo:bar"],
            expected_out="host_y64/bar\n",
            expected_err="NOTE: Auto-mapped Bazel target '//tools/foo:bar' to GN wrapper 'fx build --host //build/bazel/host:bazel_root_host_tools.bar'.\n",
        )

        # Test Bazel platform target auto-mapping from bazel_target_infos.json
        self._ninja_outputs["//src/devices/board:vim3_wrapper"] = [
            "obj/src/devices/board/vim3_wrapper.stamp"
        ]
        _write_json(self._build_dir / "ninja_outputs.json", self._ninja_outputs)
        _write_json(
            self._build_dir / "bazel_target_infos.json",
            [
                {
                    "bazel_target": "//src/devices/board:vim3",
                    "stamp_path": "obj/src/devices/board/vim3_wrapper.stamp",
                }
            ],
        )
        self.assert_output(
            ["gn_label_to_ninja_paths", "//src/devices/board:vim3"],
            expected_out="obj/src/devices/board/vim3_wrapper.stamp\n",
            expected_err="NOTE: Auto-mapped Bazel target '//src/devices/board:vim3' to GN wrapper 'fx build //src/devices/board:vim3_wrapper'.\n",
        )

        # Test fallback when wrapper exists in JSON but not in Ninja outputs
        _write_json(
            self._build_dir / "bazel_root_targets.json",
            [
                {
                    "bazel_label": "//tools/unbuilt:tool",
                    "host_bin_label": "//build/bazel/host:unbuilt.tool(//build/toolchain:host_y64)",
                }
            ],
        )
        self.assert_error(
            ["gn_label_to_ninja_paths", "//tools/unbuilt:tool"],
            "ERROR: Unknown GN label (not in the configured graph): //tools/unbuilt:tool\n"
            "NOTE: '//tools/unbuilt:tool' is a Bazel target wrapped by GN.\n"
            "      Did you mean: fx build --host //build/bazel/host:unbuilt.tool\n"
            "      Or for direct Bazel: fx build --host @//tools/unbuilt:tool\n",
        )

        # Test shorthand label qualification (e.g. //tools/foo matching //tools/foo:foo)
        _write_json(
            self._build_dir / "bazel_root_targets.json",
            [
                {
                    "bazel_label": "//tools/foo:foo",
                    "host_bin_label": "//build/bazel/host:bazel_root_host_tools.foo(//build/toolchain:host_y64)",
                }
            ],
        )
        self.assert_error(
            ["gn_label_to_ninja_paths", "//tools/foo"],
            "ERROR: Unknown GN label (not in the configured graph): //tools/foo\n"
            "NOTE: '//tools/foo' is a Bazel target wrapped by GN.\n"
            "      Did you mean: fx build --host //build/bazel/host:bazel_root_host_tools.foo\n"
            "      Or for direct Bazel: fx build --host @//tools/foo:foo\n",
        )

    def test_gn_labels_to_ninja_paths_malformed_bazel_json(self) -> None:
        # Write corrupted/non-list JSON and verify graceful fallback
        _write_file(
            self._build_dir / "bazel_root_targets.json", "NOT_VALID_JSON{"
        )
        _write_json(
            self._build_dir / "bazel_target_infos.json", {"not": "a list"}
        )
        self.assert_error(
            ["gn_label_to_ninja_paths", "//unknown:label"],
            "ERROR: Unknown GN label (not in the configured graph): //unknown:label\n",
        )

    def test_fx_build_args_to_labels(self) -> None:
        _TEST_CASES = [
            (["--args", "//aa"], ["//aa:aa"]),
            (
                ["--args", "--host", "//foo/bar"],
                ["//foo/bar:bar(//build/toolchain:host_y64)"],
            ),
            (["--args", "--fuchsia", "//:foo"], ["//:foo"]),
            (
                [
                    "--args",
                    "--host",
                    "//first",
                    "//second",
                    "--fuchsia",
                    "//third",
                    "//fourth",
                    "--fidl",
                    "//fifth",
                ],
                [
                    "//first:first(//build/toolchain:host_y64)",
                    "//second:second(//build/toolchain:host_y64)",
                    "//third:third",
                    "//fourth:fourth",
                    "//fifth:fifth(//build/fidl:fidling)",
                ],
            ),
            (
                [
                    "--args",
                    "//unknown",
                    "//other:unknown",
                ],
                ["//unknown:unknown", "//other:unknown"],
            ),
        ]
        for args, expected_list in _TEST_CASES:
            expected_out = "\n".join(expected_list) + "\n"
            self.assert_output(["fx_build_args_to_labels"] + args, expected_out)

        _WARNING_CASES = [
            (
                [
                    "--args",
                    "host_y64/hammer",
                ],
                ["//tools:hammer(//build/toolchain:host_y64)"],
                "WARNING: Use '--host //tools:hammer' instead of Ninja path 'host_y64/hammer'\n",
            ),
            (
                [
                    "--allow-targets",
                    "--args",
                    "hammer",
                ],
                ["//tools:hammer(//build/toolchain:host_y64)"],
                "WARNING: Use '--host //tools:hammer' instead of Ninja target 'hammer'\n",
            ),
            (
                [
                    "--args",
                    "foo:foo",
                ],
                ["//foo:foo"],
                "WARNING: Use '//foo' instead of 'foo:foo' for GN targets\n",
            ),
        ]
        for args, expected_list, expected_err in _WARNING_CASES:
            expected_out = "\n".join(expected_list) + "\n"
            self.assert_output(
                ["fx_build_args_to_labels"] + args,
                expected_out,
                expected_err=expected_err,
                expected_status=0,
            )

        _ERROR_CASES = [
            (
                [
                    "--args",
                    "host_y64/unknown",
                ],
                "ERROR: Unknown Ninja path: host_y64/unknown\n",
            ),
            (
                [
                    "--allow-targets",
                    "--args",
                    "first_path",
                    "second/path",
                ],
                "ERROR: Unknown Ninja target: first_path\n"
                + "ERROR: Unknown Ninja path: second/path\n",
            ),
        ]
        self.maxDiff = 1000
        for args, expected_err in _ERROR_CASES:
            self.assert_error(
                ["fx_build_args_to_labels"] + args,
                expected_err=expected_err,
            )

    def test_last_ninja_artifacts(self) -> None:
        self._build_ninja_path.write_text(
            """
rule copy
  command = cp -f $in $out

build out1: copy input1
build out2: copy out1
build out3: copy out1
build $:default: phony out1
build all: phony out1 out2 out3
"""
        )

        def assert_last_ninja_artifacts_output(expected: str) -> None:
            self.assert_output(["last_ninja_artifacts"], expected)

        # Verify that if the file doesn't exist, then the result should
        # correspond to the :default target.
        assert not self._last_targets_path.exists()
        assert_last_ninja_artifacts_output("out1\n")

        # Change the list of targets.
        self._last_targets_path.write_text("all")
        assert_last_ninja_artifacts_output("out1\nout2\nout3\n")

    def test_export_last_build_debug_symbols(self) -> None:
        self.maxDiff = None

        self._build_ninja_path.write_text(
            """
rule whatever
  command = ignored

build obj/src/foo/lib_shared/libfoo.so.unstripped obj/src/bar/binary.unstripped obj/src/zoo/binary.unstripped: whatever ../../prebuilt/.build-id/aa/bbbbbbbbbbb.debug

build $:default: phony obj/src/foo/lib_shared/libfoo.so.unstripped
"""
        )

        # Create a fake dump_syms tool that simply prints the path of
        # the input debug symbol file.
        dump_syms = self._top_dir / "dump_syms"
        dump_syms.write_text(
            f"""#!{sys.executable}
import argparse

parser = argparse.ArgumentParser()
parser.add_argument("-r", action="store_true")
parser.add_argument("-n")
parser.add_argument("-o")
parser.add_argument("debug_symbol_file")

args = parser.parse_args()

print(args.debug_symbol_file)
"""
        )
        dump_syms.chmod(0o755)

        gsymutil = self._top_dir / "gsymutil"
        gsymutil.write_text(
            f"""#!{sys.executable}
import argparse
import os

parser = argparse.ArgumentParser()
parser.add_argument("--convert", required=True, help="Input debug binary")
parser.add_argument("--out-file", required=True, help="Output file path")
args = parser.parse_args()

os.makedirs(os.path.dirname(args.out_file), exist_ok=True)
with open(args.out_file, "wt") as f:
    f.write(args.convert)
    f.write("\\n")
"""
        )
        gsymutil.chmod(0o755)

        export_dir = self._top_dir / "exported_debug_symbols"

        expected_err = """MISSING build-id FOR {'cpu': 'x64', 'debug': 'obj/src/zoo/binary.unstripped', 'label': '//src/zoo:binary', 'os': 'fuchsia'}
"""

        expected_out = f"""Creating {export_dir}/build-ids.json
Creating {export_dir}/build-ids.txt
Creating 3 symlinks in {export_dir}
Generating 3 breakpad symbols in {export_dir}
  - Creating .build-id/00/000000000000001.sym FROM obj/src/foo/lib_shared/libfoo.so.unstripped
  - Creating .build-id/aa/bbbbbbbbbbb.sym FROM ../../prebuilt/.build-id/aa/bbbbbbbbbbb.debug
  - Creating .build-id/bu/ild_id_for_bar.sym FROM obj/src/bar/binary.unstripped
Generating 3 GSYM symbols in {export_dir}
  - Creating .build-id/00/000000000000001.gsym FROM obj/src/foo/lib_shared/libfoo.so.unstripped
  - Creating .build-id/aa/bbbbbbbbbbb.gsym FROM ../../prebuilt/.build-id/aa/bbbbbbbbbbb.debug
  - Creating .build-id/bu/ild_id_for_bar.gsym FROM obj/src/bar/binary.unstripped
Done!
"""
        self.assert_output(
            [
                "export_last_build_debug_symbols",
                f"--output-dir={export_dir}",
                "--with-breakpad-symbols",
                f"--dump_syms={dump_syms}",
                "--with-gsym-symbols",
                f"--gsymutil={gsymutil}",
            ],
            expected_out,
            expected_err,
        )

    def test_target_metadata(self) -> None:
        self.maxDiff = None

        # Create project.json in build_dir
        _write_json(
            self._build_dir / "project.json",
            {
                "targets": {
                    "//foo:bar": {
                        "deps": ["//baz:qux"],
                        "sources": ["//foo/bar.cc"],
                        "inputs": ["//foo/bar.h"],
                    },
                    "//foo:baz": {
                        "deps": [],
                        "sources": ["//foo/baz.cc"],
                        "inputs": [],
                    },
                }
            },
        )

        output_file = self._top_dir / "target_metadata.json"

        # Test command execution
        ret = self.run_client(
            [
                "target_metadata",
                f"--output={output_file}",
            ]
        )

        self.assertEqual(
            ret.returncode, 0, msg=f"Command failed with:\n{ret.stderr}"
        )

        expected_metadata = {
            "$schema": "target_metadata.schema.json",
            "version": 1,
            "targets": {
                "//foo:bar": {
                    "deps": ["//baz:qux"],
                    "sources": ["foo/bar.cc"],
                    "inputs": ["foo/bar.h"],
                    "source_dir": "foo",
                },
                "//foo:baz": {
                    "deps": [],
                    "sources": ["foo/baz.cc"],
                    "inputs": [],
                    "source_dir": "foo",
                },
            },
        }

        self.assertTrue(output_file.exists())
        with output_file.open("r") as f:
            actual_metadata = json.load(f)
        self.assertEqual(actual_metadata, expected_metadata)


class ShouldFileChangesTriggerBuildClientTest(ClientTestBase):
    def setUp(self) -> None:
        super().setUp()
        self._build_ninja_d_path.write_text(
            "build.ninja.stamp: ../../BUILD.gn host_x64/ignore.me ../../src/template.gni\n"
        )

        # The build_api_client_info file maps each module name to its .json file.
        _write_file(
            self._build_dir / "build_api_client_info",
            """args=args.json
build_info=build_info.json
""",
        )

        # Ninja build plan that matches the following diagram
        #
        #    //foo.cc    //foo.h   //bar.h  //bar.cc
        #       |         |   |        |        |
        #       |         |   |        |        |
        #       ===========   ===================
        #            |                 |
        #       obj/foo.cc.o        obj/bar.cc.o
        #
        #
        #    //src/lib.cc
        #         |
        #         |
        #       =====
        #         |
        #    obj/src/lib.cc.o    //src/main.cc
        #         |                    |
        #         |                    |
        #         ======================
        #            |              |
        #      obj/src/program   obj/src/min.cc.o
        #            |
        #            |
        #           ===
        #            |
        #         :default
        #
        self._build_ninja_path.write_text(
            r"""
rule compile
    command = touch $out

rule link
    command = touch $out

build obj/foo.cc.o: compile ../../foo.cc ../../foo.h

build obj/bar.cc.o: compile ../../bar.cc ../../bar.h ../../foo.h

build obj/src/lib.cc.o: compile ../../src/lib.cc

build obj/src/main.cc.o obj/src/program: link ../../src/main.cc obj/src/lib.cc.o

build $:default: phony obj/src/program

default $:default
"""
        )

        # Fake Ninja outputs matching the build plan above.
        self._ninja_outputs = {
            "//:foo": [
                "obj/foo.cc.o",
            ],
            "//:bar": [
                "obj/bar.cc.o",
            ],
            "//src:lib": [
                "obj/src/lib.cc.o",
            ],
            "//src:bin": [
                "obj/src/main.cc.o",
                "obj/src/program",
            ],
        }
        _write_json(self._build_dir / "ninja_outputs.json", self._ninja_outputs)

        self._files_list_path = self._top_dir / "files_list.txt"

    def write_files_list(self, files: list[str]) -> None:
        self._files_list_path.write_text("\n".join(files))

    def _test_changed_files(
        self, changed_files: list[str], expected_out: str
    ) -> None:
        # Check passing the changed file paths directly.
        self.assert_output(
            args=["should_file_changes_trigger_build"] + changed_files,
            expected_out=expected_out,
            msg=f"for changed files {changed_files}",
        )

        # Check using --files-list.
        self.write_files_list(changed_files)
        self.assert_output(
            args=[
                "should_file_changes_trigger_build",
                f"--files-list={self._files_list_path}",
            ],
            expected_out=expected_out,
            msg=f"for changed files {changed_files}",
        )

    def test_no_changes_needed(self) -> None:
        TEST_CASES: T.Sequence[list[str]] = (
            # No changed files at all.
            [],
            # Changed files are sources that are not inputs in the current build plan.
            ["src/other.cc"],
            # Chagned files are build files that are used not used by the current GN graph.
            ["other/BUILD.gn", "other/template.gni"],
        )

        for changed_files in TEST_CASES:
            self._test_changed_files(changed_files, "NO\n")

    def test_build_file_changes(self) -> None:
        TEST_CASES = (
            # Main build file changed.
            ["BUILD.gn"],
            # Unrelated and main build file changed.
            ["other/BUILD.gn", "BUILD.gn"],
            # Main .gni file changed.
            ["src/template.gni"],
            # Unrelated and main .gni file changed.
            ["other/template.gni", "src/template.gni"],
            # Unrelated and main build and .gni files changed.
            [
                "BUILD.gn",
                "other/BUILD.gn",
                "src/template.gni",
                "other/template.gni",
            ],
        )
        for changed_files in TEST_CASES:
            self._test_changed_files(
                changed_files, "YES: GN build graph changed.\n"
            )

    def test_source_file_changes(self) -> None:
        TEST_CASES: T.Sequence[tuple[list[str], str]] = (
            # Sources that are dependencies of :default should trigger a rebuild.
            (["src/lib.cc"], "YES: Sources updated for target: :default\n"),
            # Sources that are not dependencies of :default should not trigger a rebuild.
            (["bar.cc"], "NO\n"),
            # Sources that are inputs for different targets.
            (
                ["bar.cc", "src/main.cc"],
                "YES: Sources updated for target: :default\n",
            ),
        )
        for changed_files, expected_out in TEST_CASES:
            self._test_changed_files(changed_files, expected_out)

        # Now make 'foo' and 'bar' the last build's targets.
        self._last_targets_path.write_text("obj/foo.cc.o obj/bar.cc.o\n")

        TEST_CASES = (
            # Sources that are dependencies of foo or bar should trigger a rebuild.
            (["bar.cc"], "YES: Sources updated for target: obj/bar.cc.o\n"),
            (["foo.cc"], "YES: Sources updated for target: obj/foo.cc.o\n"),
            (["foo.cc", "bar.cc"], "YES: Sources updated for 2 targets.\n"),
            (["foo.h"], "YES: Sources updated for 2 targets.\n"),
            # Sources that are not dependencies of foo or bar should not trigger a rebuild.
            (["src/lib.cc"], "NO\n"),
            # Changed to build files and sources should report build change only.
            (["BUILD.gn", "bar.cc"], "YES: GN build graph changed.\n"),
        )
        for changed_files, expected_out in TEST_CASES:
            self._test_changed_files(changed_files, expected_out)


class AffectedTestsClientTest(ClientTestBase):
    def setUp(self) -> None:
        super().setUp()

        _write_file(
            self._build_dir / "build.ninja",
            r"""
rule compile
    command = touch $out

rule link
    command = touch $out

rule test_package
    command = touch $out

build obj/src/lib.cc.o: compile ../../src/lib.h ../../src/lib.cc

build obj/src/program obj/src/main.cc.o: link ../../src/main.cc obj/src/lib.cc.o

build obj/src/program2 obj/src/main2.cc.o: link ../../src/main2.cc obj/src/lib.cc.o

build obj/src/host_test obj/src/host_test.cc.o: link ../../src/host_test.cc

build obj/packages/foo/package_manifest.json: test_package ../../packages/foo/component.cml | obj/packages/foo.runtime_deps.json || obj/src/program2 ../../tools/test_runner.sh

build obj/packages/bar/package_manifest.json: test_package ../../packages/bar/component.cml | obj/packages/bar.package_manifest_deps.json || ../../tools/test_runner.sh

build host_test: phony obj/src/host_test
build foo_test: phony obj/packages/foo/package_manifest.json

build $:default: phony host_test foo_test

default $:default
""",
        )

        _write_json(
            self._build_dir / "ninja_outputs.json",
            {
                "//src:host_test(//toolchain:host)": [
                    "obj/src/host_test",
                    "obj/src/host_test.cc.o",
                ],
                "//src:program(//toolchain:host)": [
                    "obj/src/program",
                    "obj/src/main.cc.o",
                ],
                "//src:program2(//toolchain:host)": [
                    "obj/src/program2",
                    "obj/src/main2.cc.o",
                ],
                "//src:lib(//toolchain:host)": ["obj/src/lib.cc.o"],
                "//packages:foo(//toolchain:device)": [
                    "obj/packages/foo/package_manifest.json",
                ],
                "//packages:bar(//toolchain:device)": [
                    "obj/packages/bar/package_manifest.json",
                ],
            },
        )

        _write_json(
            self._build_dir / "tests.json",
            [
                # A host test that depends on //src:program at runtime too.
                {
                    "test": {
                        "label": "//src:host_test(//toolchain:host)",
                        "path": "obj/src/host_test",
                        "runtime_deps": "obj/src/host_test.runtime_deps.json",
                        "os": "linux",
                    }
                },
                # A device test package that depends on //src:program2 at runtime.
                {
                    "test": {
                        "label": "//packages:foo(//toolchain:device)",
                        "new_path": "../../tools/test_runner.sh",
                        "runtime_deps": "obj/packages/foo.runtime_deps.json",
                        "package_manifests": [
                            "obj/packages/foo/package_manifest.json",
                        ],
                        "os": "fuchsia",
                    }
                },
                # A device test package that depends on //package:foo at runtime,
                # and thus transitively on //src:program2
                {
                    "test": {
                        "label": "//packages:bar(//toolchain:device)",
                        "new_path": "../../tools/test_runner.sh",
                        "package_manifests": [
                            "obj/packages/bar/package_manifest.json",
                        ],
                        "package_manifest_deps": "obj/packages/bar.package_manifest_deps.json",
                        "os": "fuchsia",
                    }
                },
            ],
        )

        _write_json(
            self._build_dir / "obj/src/host_test.runtime_deps.json",
            [
                "obj/src/program",
            ],
        )

        _write_json(
            self._build_dir / "obj/packages/foo.runtime_deps.json",
            [
                "obj/src/program2",
            ],
        )

        _write_json(
            self._build_dir / "obj/packages/bar.package_manifest_deps.json",
            [
                "obj/packages/foo/package_manifest.json",
            ],
        )

        self._files_list_path = self._top_dir / "files_list.txt"

    def write_list_file(self, paths: list[str]) -> Path:
        _write_file(self._files_list_path, "\n".join(paths))
        return self._files_list_path

    def test_affected_test(self) -> None:
        # Modifying a source file that was not used by the last build doesn't affect anything.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["src/other.h"]),
            ],
            "",
        )

        # Modifying the host test source should affect it.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["src/host_test.cc"]),
            ],
            "//src:host_test(//toolchain:host),host\n",
        )

        # Modifying the //src/main.cc source file should affect the host test.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["src/main.cc"]),
            ],
            "//src:host_test(//toolchain:host),host\n",
        )

        # Modifying the //src/libc.cc source file should affect the host test and
        # the device tests because it is also used by //src:program2
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["src/lib.cc"]),
            ],
            "//packages:bar(//toolchain:device),device\n"
            + "//packages:foo(//toolchain:device),device\n"
            + "//src:host_test(//toolchain:host),host\n",
        )

        # Modifying the bar component manifest only affects the bar test package.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["packages/bar/component.cml"]),
            ],
            "//packages:bar(//toolchain:device),device\n",
        )

        # Modifying the foo component manifest should affect the foo test package
        # but also the bar test package that depends on it. Due to dependency ordering
        # bar appears before foo.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["packages/foo/component.cml"]),
            ],
            "//packages:bar(//toolchain:device),device\n"
            "//packages:foo(//toolchain:device),device\n",
        )

        # Modifying //src:program2 source file should also affect the foo test package
        # and the bar one.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["src/main2.cc"]),
            ],
            "//packages:bar(//toolchain:device),device\n"
            "//packages:foo(//toolchain:device),device\n",
        )

        # Modifying the test runner script affects both test packages.
        self.assert_output(
            [
                "affected_tests",
                "--files-list",
                self.write_list_file(["tools/test_runner.sh"]),
            ],
            "//packages:bar(//toolchain:device),device\n"
            "//packages:foo(//toolchain:device),device\n",
        )


if __name__ == "__main__":
    unittest.main()
