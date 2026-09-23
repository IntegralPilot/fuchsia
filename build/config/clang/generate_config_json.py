#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate clang_config.json file from a given Clang toolchain install path.

This probes the Clang toolchain by invoking it many times with different command-line
arguments such as --print-file-name=<name>. All commands are run in parallel to speed
the result, use -j<count> or --jobs=<count> to change that.
"""

import argparse
import dataclasses
import json
import os
import subprocess
import sys
import time
import typing as T
from pathlib import Path

# TECHNICAL NOTE:
#
# IMPORTANT: The schema described here is an implementation detail of the
# Fuchsia build system, and may change widely over time. DO NOT DEPEND ON
# IT FOR ANY OTHER WORKFLOWS.
#
# For context, see go/clang-runtime-integration-for-the-fuchsia-build
# and go/clang-and-rust-toolchain-requirements-for-fuchsia-builds
#
# This outputs a JSON file that consists in a single JSON object with
# the following schema:
#
#  "<clang_target_key>": {
#    "resource_dir": "<resource_dir>",
#    "libunwind_so": "<base_libunwind_so>",
#    "libclang_rt_profile_a": "<libclang_rt_profile_a>",
#    "variants": {
#       "<variant>": {
#          "static": {
#             "<libname>": "<relative path to static library file, or empty string if not available."
#             ... for different <libname> values in [ "clang_rt", "clang_rt_cxx" ]
#          },
#          "shared": {
#             "<libname>": "<relative path to shared library file, or empty string if not available."
#             ... for different <libname> values in [ "clang_rt", "libcxx" ]
#             "libcxx": "...",
#             "libcxx_abi": "...",
#             "libunwind": "...",
#          },
#       },
#       ... for different <variant> values.
#    }
#  },
#  "runtimes": <runtimes>
#
# Where all paths are relative to the Clang installation directory, and where:
#
# - <runtimes> is the content of the $clang_dir/lib/runtime.json which is currently
#   a list of scopes. See //build/toolchain/runtime/toolchain_runtime_deps.gni for details.
#
# - <clang_target_key> is a clang target tuple, with all dashes replaced
#   with underscores, to make them readable by GN (e.g. "x86_64_unknown_fuchsia"
#   is the key for "x86_64-unknown-fuchsia").
#
#   There will be a special <clang_target_key> named "fallback" that will be used
#   as a fallback used for non-C++ GN toolchain instances, or for the unknown-unknown-wasm32
#   toolchain. Its `resource_dir` and `include_cxx_v1` fields will be valid, all other values
#   will be empty, with only the `none` <variant> being supported.
#
# - <resource_dir> is the path to the Clang resource directory
#   (e.g. "lib/clang/20"")
#
# - <base_libunwdind_so> is the path to the regular libunwind.so for the current
#   clang target tuple, or the empty string if there is none.
#
# - <libclang_rt_profile_a> is the relative path to the Clang profile runtime static library
#   (used to implement the "profile" build variant that generates instrumented code to collect
#   profiling information). Or the empty string if none is available.
#
#   NOTE: Used by //build/config/profile/BUILD.gn
#
# - <variant> is a variant name from a subset of [ "none", "ubsan", "asan", "hwasan", "tsan", "lsan" ]
#   which depends on <clang_target_key>. For example, `x86_64_pc_windows_msvc` only includes
#   one entry in ["x86_64_pc_windows_msvc]["variants] only includes one entry for the "none" key.
#

_IMPORT_DIR = Path(__file__).parent.parent.parent / "images"
sys.path.append(str(_IMPORT_DIR))

_FUCHSIA_TARGETS = [
    "aarch64-unknown-fuchsia",
    "riscv64-unknown-fuchsia",
    "x86_64-unknown-fuchsia",
]

_HOST_TARGETS = [
    "armv7-unknown-linux-gnueabihf",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "riscv64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
]

_ALL_TARGETS = _FUCHSIA_TARGETS + _HOST_TARGETS + ["fallback"]


class Runtime(T.TypedDict):
    cflags: list[str]
    ldflags: list[str]
    runtime: list[dict[str, str]]
    target: list[str]


def fix_clang_runtime_json(runtime_json: list[Runtime]) -> None:
    """Fix the content of runtime.json."""

    # As a special case, on Fuchsia and for ASan builds, libclang_rt.asan.so depends
    # on libc++abi.so even if the generated machine code does not (i.e. when
    # -static-libstdc++ is also used). libc++abi.so depends on libunwind.so itself.
    # Linking issues appear because these dependencies are not listed properly, so
    # work-around this by patching runtime.json for Asan.

    # This is also true for other sanitizer runtime shared libraries.

    # For example, given an input entry like:
    #
    #  {
    #    cflags = [ "-fsanitize=address" ],
    #    ldflags = [ "-static-libstdc++" ],
    #    runtime = [ <runtimes> ],
    #    target = [ "x86_64-unknown-fuchsia", "x86_64-fuchsia" ],
    #  },
    #
    # The <runtimes> value above must be replaced with the one from the
    # corresponding non-static C++ runtime entry, that looks like:
    #
    #  {
    #    cflags = [ "-fsanitize=address" ],
    #    ldflags = [],
    #    runtime = [ <runtimes> ],
    #    target = [ "x86_64-unknown-fuchsia", "x86_64-fuchsia" ],
    #  },
    #

    # This only applies to Fuchsia entries.
    fuchsia_entries = [
        entry for entry in runtime_json if "-fuchsia" in entry["target"][0]
    ]

    def match_static_libstdcxx(entry: Runtime) -> bool:
        return "-static-libstdc++" in entry["ldflags"]

    def match_sanitizer(entry: Runtime) -> bool:
        return any(
            cflags.startswith("-fsanitize=") for cflags in entry["cflags"]
        )

    static_entries = [e for e in fuchsia_entries if match_static_libstdcxx(e)]
    shared_entries = [
        e for e in fuchsia_entries if not match_static_libstdcxx(e)
    ]

    for static in static_entries:
        # For sanitized modes, find the corresponding shared entry with the same cflags and
        # target values, then update the "runtime" value.
        if match_sanitizer(static):
            for shared in shared_entries:
                if (
                    shared["cflags"] == static["cflags"]
                    and shared["target"] == static["target"]
                ):
                    static["runtime"] = shared["runtime"]
                    break


def get_clang_target_variants(clang_target: str) -> T.Sequence[str]:
    """Get the variants to support for a given clang_target tuple."""
    if "fallback" in clang_target:
        return ("none",)
    if "-windows-" in clang_target:
        # These are only used for the EFI bootloaders, whose toolchains
        # do not support variants.
        return ("none",)
    if "wasm32" in clang_target:
        # The wasm32 GN toolchain does not support variants.
        return ("none",)
    # Default list of variants to support.
    return ("none", "ubsan", "asan", "hwasan", "lsan", "tsan")


def get_clang_variant_flags(variant: str) -> list[str]:
    """Return the Clang flag used to enable a given variant."""
    return {
        "asan": ["-fsanitize=address"],
        "hwasan": ["-fsanitize=hwaddress"],
        "lsan": ["-fsanitize=leak"],
        "tsan": ["-fsanitize=thread"],
        "ubsan": ["-fsanitize=undefined"],
    }.get(variant, [])


def get_clang_rt_library_name(variant: str, ext: str, suffix: str = "") -> str:
    """Return the name of a variant's Clang runtime library file."""
    runtime_name = {"ubsan": "ubsan_standalone"}.get(variant, variant)
    return f"libclang_rt.{runtime_name}{suffix}{ext}"


def get_shared_static_exts(clang_target: str) -> tuple[str, str]:
    """Return a pair of file extensions for shared and static libraries."""
    if "-windows-" in clang_target:
        return ".dll", ".lib"
    elif "-apple-" in clang_target:
        return "_osx_dynamic.dylib", ".a"
    elif "-wasm" in clang_target:
        return ".wasm", ".a"
    else:
        return ".so", ".a"


Tree = dict[str, T.Union[str, "Tree"]]


def store_into_dict(d: Tree, path: str, value: str) -> None:
    """Store a value into a tree of string-keyed dictionaries.

    Args:
        d: The target directory, will be modified in-place.
        path: A string containing a dot-separate list of key values
               e.g. "foo.bar" would be used to set d["foo"]["bar"].
        value: The value to set.
    """
    components = path.split(".")
    assert len(components) > 0, "Trying to use an empty path!"

    curr_dict = d
    for curr_field in components[:-1]:
        assert curr_field, f"Empty component in {path=}"
        next_dict = curr_dict.setdefault(curr_field, {})
        assert isinstance(
            next_dict, dict
        ), f"Inconsistent component in {path=}, {next_dict=}"
        curr_dict = next_dict

    curr_dict[components[-1]] = value


def dict_to_gn_scope_inplace(v: Tree) -> None:
    """Convert a GN scope's object keys to be compatible with GN.

    GN cannot read JSON objects whose keys are not valid GN identifiers.
    For this script, this requires replacing dashes with underscores.
    """
    keys = list(v.keys())
    for key in keys:
        value = v[key]
        new_key = key.replace("-", "_")
        if new_key != key:
            v[new_key] = value
            del v[key]
        if isinstance(value, dict):
            dict_to_gn_scope_inplace(value)


class CommandResult(object):
    """A class used to process a command's invocation result.

    The process() method will be called to extract a result string value
    from a given Popen value (for a completed command).

    By default, command failure returns an empty string, and command success
    returns the stripped stdout.

    Sub-classes should override the process_error() and process_stdout()
    methods if they want to apply transformations, e.g. rebasing paths.
    """

    def __init__(self) -> None:
        pass

    def process(self, proc: "subprocess.Popen[str]") -> str:
        if proc.returncode != 0:
            return self.process_error(proc)
        else:
            assert proc.stdout is not None
            return self.process_stdout(proc.stdout.read().strip())

    def process_error(self, proc: "subprocess.Popen[str]") -> str:
        """This method can be overridden by sub-classes"""
        return ""

    def process_stdout(self, stdout: str) -> str:
        """This method can be overridden by sub-classes"""
        return stdout


class ClangRelativePathResult(CommandResult):
    """A CommandResult sub-class that simply converts a path to a clang-relative one."""

    def __init__(self, clang_dir: Path) -> None:
        super().__init__()
        self._clang_dir = clang_dir.resolve()

    def process_stdout(self, path: str) -> str:
        if not path:
            # Empty paths are returned by --print-file-path when Clang does not
            # find a file.
            return path
        path = os.path.realpath(path)
        return os.path.relpath(path, self._clang_dir)


class ClangFileNamePathResult(ClangRelativePathResult):
    """A CommandResult sub-class used to process the output of --print-file-name."""

    def __init__(self, filename: str, clang_dir: Path) -> None:
        super().__init__(clang_dir)
        self._filename = filename

    def process_stdout(self, path: str) -> str:
        if path == self._filename:
            # The unmodified filename is returned by --print-file-name if
            # the file is not found.
            return ""
        else:
            # Otherwise, make the result relative to the Clang directory.
            return super().process_stdout(path)


@dataclasses.dataclass
class CommandInfo(object):
    """Record the state of a command in a CommandPool instance."""

    cmd_args: list[str]
    cmd_result: CommandResult


class CommandPool(object):
    """Implement a pool of parallel commands.

    Usage is:
       - Create instance.
       - Call add_command() as many times as necessary.
       - Call run() to iterate over (dest, value) pairs.
    """

    def __init__(self, pool_depth: int = 16) -> None:
        """Initialize instance.

        Args:
           pool_depth: Maximum number of commands that will be run in parallel.
        """
        self._commands: dict[str, CommandInfo] = {}
        self._depth = pool_depth

    def add_command(
        self, dest: str, cmd_result: CommandResult, cmd_args: list[str]
    ) -> None:
        """Add new command to the pool.

        Args:
            dest: A unique name for this command invocation. This will
                be included in the tuple yielded from run().

            cmd_result: A CommandResult instance whose process() method
                will be called on command completion. Its result will
                be included in the tuple yielded from run().

            cmd_args: The command arguments passed to subprocess.run().
        """
        self._commands[dest] = CommandInfo(cmd_args, cmd_result)

    def run(self) -> T.Iterator[tuple[str, str]]:
        """Run all queued commands in parallel.

        Yields:
            (dest, value) pairs, where |dest| is the |dest| parameter
            passed to add_command(), and |value| is the result of
            calling the corresponding CommandResult.process() method.
        """
        running: dict[str, subprocess.Popen[str]] = {}

        def poll_run_queue() -> T.Iterator[tuple[str, str]]:
            completed = []
            for dest, proc in running.items():
                returncode = proc.poll()
                if returncode is not None:
                    completed.append((dest, info.cmd_result.process(proc)))

            # Release file descriptors as early as possible.
            del proc  # Python does not have lexical scopes. 🤦
            for dest, _ in completed:
                del running[dest]

            yield from completed

            if not completed:
                time.sleep(0.01)  # 10ms

        for dest, info in self._commands.items():
            while len(running) == self._depth:
                yield from poll_run_queue()

            running[dest] = subprocess.Popen(
                info.cmd_args,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )

        while running:
            yield from poll_run_queue()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--clang-dir",
        type=Path,
        required=True,
        help="Path to Clang installation directory",
    )

    core_count = os.cpu_count()
    if core_count is None:
        core_count = 1

    default_jobs = core_count

    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=default_jobs,
        help=f"Number of parallel commands to run (default {default_jobs}).",
    )

    parser.add_argument(
        "--to-gn-scope", action="store_true", help="Convert result to GN scope."
    )
    parser.add_argument(
        "--pretty", action="store_true", help="Print human-readable output."
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output file path. Defaults prints to stdout.",
    )
    args = parser.parse_args()

    if args.jobs <= 0:
        parser.error("Job count must be strictly positive!")

    clang_bin = args.clang_dir / "bin" / "clang"
    if not clang_bin.exists():
        parser.error(f"Missing Clang binary: {clang_bin} (cwd={os.getcwd()})")

    clangxx_bin = args.clang_dir / "bin" / "clang++"
    if not clangxx_bin.exists():
        parser.error(
            f"Missing Clang++ binary: {clangxx_bin} (cwd={os.getcwd()})"
        )

    # Read lib/runtime.json file.
    runtime_json_path = args.clang_dir / "lib" / "runtime.json"
    if not runtime_json_path.exists():
        parser.error(f"Missing input file: {runtime_json_path}")
    with runtime_json_path.open("rb") as f:
        runtime_json = json.load(f)

    result: Tree = {}

    command_pool = CommandPool(args.jobs)

    clang_relative_path_result = ClangRelativePathResult(args.clang_dir)

    # TODO(https://fxbug.dev/42135607): Get this information from runtime.json or
    # equivalent instead.
    for clang_target in _ALL_TARGETS:
        clang_target_cmd_prefix = [clang_bin, f"--target={clang_target}"]
        clangxx_target_cmd_prefix = [
            clangxx_bin,
            f"--target={clang_target}",
            # "-fno-exceptions",
        ]

        shared_ext, static_ext = get_shared_static_exts(clang_target)

        command_pool.add_command(
            f"{clang_target}.resource_dir",
            clang_relative_path_result,
            cmd_args=(clang_target_cmd_prefix + ["--print-resource-dir"]),
        )

        def get_clangxx_file_path(dest: str, filename: str) -> None:
            command_pool.add_command(
                f"{clang_target}.{dest}",
                ClangFileNamePathResult(filename, args.clang_dir),
                clangxx_target_cmd_prefix + [f"--print-file-name={filename}"],
            )

        # LINT.IfChange
        get_clangxx_file_path("libunwind_so", f"libunwind{shared_ext}")
        # LINT.ThenChange(//build/config/fuchsia/BUILD.gn)

        # LINT.IfChange
        get_clangxx_file_path(
            "libclang_rt_profile_a", f"libclang_rt.profile{static_ext}"
        )
        # LINT.ThenChange(//build/config/profile/BUILD.gn)

        # LINT.IfChange
        for variant in get_clang_target_variants(clang_target):
            if variant != "none":
                get_clangxx_file_path(
                    f"variants.{variant}.shared.clang_rt",
                    get_clang_rt_library_name(variant, shared_ext),
                )

                get_clangxx_file_path(
                    f"variants.{variant}.static.clang_rt",
                    get_clang_rt_library_name(variant, static_ext),
                )

                get_clangxx_file_path(
                    f"variants.{variant}.static.clang_rt_cxx",
                    get_clang_rt_library_name(variant, static_ext, "_cxx"),
                )
        # LINT.ThenChange(//build/config/sanitizers/BUILD.gn)

    for dest, value in command_pool.run():
        store_into_dict(result, dest, value)

    def get_dest_path(variant: str, soname: str) -> str:
        return f"lib/{variant}/{soname}"

    # Add empty variants dictionary to all targets that don't have one.
    # This simplifies the GN code paths.
    for clang_target, values in result.items():
        assert isinstance(values, dict)
        values.setdefault("variants", {})

    # LINT.IfChange
    fix_clang_runtime_json(runtime_json)
    result["runtimes"] = runtime_json
    # LINT.ThenChange(//build/toolchain/runtime/toolchain_runtime_deps.gni)

    # For Fuchsia targets, parse the ELF binary to compute its debug and breakpad file paths,
    # which depend on its embedded GNU build-id.

    if args.to_gn_scope:
        # Replace - with _ in all keys to make the result loadable by GN
        dict_to_gn_scope_inplace(result)

    json_indent = None
    if args.pretty:
        json_indent = 2
    print(
        json.dumps(result, indent=json_indent),
        file=args.output.open("w") if args.output else sys.stdout,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
