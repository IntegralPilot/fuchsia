#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import re
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import bazel_compdb_utils

_OPT_PATTERN = re.compile("[\W]+")

_BAZEL_CPU_ALIASES = {
    "k8": "x86_64",
    "x64": "x86_64",
    "x86_64": "x86_64",
    "aarch64": "aarch64",
    "arm64": "aarch64",
    "riscv64": "riscv64",
}


def get_bazel_compilation_mode(bazel_args: dict[str, Any]) -> str:
    build_args = bazel_args.get("build", [])
    for arg in build_args:
        if arg.startswith("--compilation_mode="):
            return arg.split("=")[1]
    print(
        "WARNING: No compilation mode found in bazel args. Defaulting to fastbuild"
    )
    return "fastbuild"


def map_bazel_cpu(cpu: str) -> str | None:
    """Converts a cpu to one that bazel recognizes"""
    return _BAZEL_CPU_ALIASES.get(cpu, cpu)


_SHOULD_LOG = False

_FUCHSIA_PACKAGE_SUFFIX = "_fuchsia_package"


def canonicalize_label_from_arg(label: str) -> str:
    # fuchsia_package targets append a suffix to them which is not obvious.
    # We check the label to see if the user has appended it or not and fix
    # it for them here.
    if label.endswith(_FUCHSIA_PACKAGE_SUFFIX):
        return label
    else:
        return label + _FUCHSIA_PACKAGE_SUFFIX


def assert_arg_label_is_fuchsia_package(bazel_exe: str, label: str) -> None:
    results = collect_labels_from_scope(bazel_exe, label)
    if len(results) == 0:
        fail(
            "Provided label '{}' is not a valid fuchsia_package label. Please provide a label that points to a valid fuchsia package or use --dir instead.".format(
                label
            )
        )


def collect_labels_from_dir(args: argparse.Namespace) -> Sequence[str]:
    # Clean up the scope so it matches what bazel expects.
    dir = args.dir.removeprefix("//").removesuffix("...").removesuffix("/")
    if dir == "":
        scope = "//..."
    else:
        scope = "//{}/...".format(dir)

    return collect_labels_from_scope(args.bazel, scope)


def collect_labels_from_scope(bazel_exe: str, scope: str) -> Sequence[str]:
    try:
        return bazel_compdb_utils.run(
            bazel_exe,
            "query",
            'kind("_build_fuchsia_package(_test)? rule", {})'.format(scope),
            "--ui_event_filters=-info,-warning",
            "--noshow_loading_progress",
            "--noshow_progress",
        ).splitlines()
    except:
        fail(
            """Unable to find any labels in {}.

        This can occur when the scope is too broad and bazel tries to query
        paths that are not compatible with bazel. For example, if you try to
        query the root directory it will pick up the prebuilt directory which
        contains files that cause the query to fail.

        Try the query again with a more limited scope.
        """.format(
                scope
            )
        )
        return []  # So pytype is happy.


def fail(msg: str, exit_code: int = 1) -> None:
    print("ERROR: ", msg)
    sys.exit(exit_code)


def info(msg: str) -> None:
    if _SHOULD_LOG:
        print("INFO: ", msg)


def init_logger(verbose: bool) -> None:
    global _SHOULD_LOG
    if verbose:
        _SHOULD_LOG = True


def is_none(obj: Any) -> bool:
    return obj == None


def main(argv: Sequence[str]) -> None:
    parser = argparse.ArgumentParser(description="Refresh bazel compdb")

    parser.add_argument("--bazel", required=True, help="The bazel binary")
    parser.add_argument(
        "--build-dir", required=True, type=Path, help="The build directory"
    )
    parser.add_argument(
        "--label",
        help="The bazel label to query. This label must point to a fuchsia_package or one of its test variants.",
    )
    parser.add_argument(
        "--dir",
        help="""A directory to search for labels relative to //

        This path must be a path that we can run `fx bazel query` on. Some paths
        are not compatible with bazel queries and will fail.""",
    )
    parser.add_argument(
        "--bazel-target-infos",
        required=False,
        help="""A build API module of all Bazel target infos in this build.
        When specified, this argument takes precedence over --label and --dir.
        All Fuchsia Bazel targets from the build API module file are refreshed.""",
        type=Path,
    )
    parser.add_argument(
        "-v",
        "--verbose",
        required=False,
        help="If we should print info logs",
        default=False,
        action="store_true",
    )
    parser.add_argument(
        "--self-test-filter",
        required=False,
        help="""If provided will run a self-test on the files that match the filter.

        The self-test will attempt to compile the file given the set of arguments
        in the compile commands. This check can be very slow because it needs to
        compile every file that matches the filter. It is directly invoking clang
        do it does not benefit from the cached results. This flag should only be
        used for debugging.

        When used in conjunction with --verbose, the command will print out the
        clang errors.

        The filter will perform a re.search on the file.
        """,
        default=None,
    )
    args = parser.parse_args(argv)
    init_logger(args.verbose)

    labels: list[str] = []

    if args.bazel_target_infos:
        with open(args.bazel_target_infos, "r") as f:
            bazel_target_infos = json.load(f)
            labels = sorted(
                list(
                    {
                        t["bazel_target"]
                        for t in bazel_target_infos
                        if "bazel_target" in t
                    }
                )
            )
        if not labels:
            info(
                "No Bazel labels to refresh from {}".format(
                    args.bazel_target_infos
                )
            )
            return
    else:
        if is_none(args.label) and is_none(args.dir):
            fail("Either --label or --dir must be set.")

        if args.label:
            label = canonicalize_label_from_arg(args.label)
            info("Verifying label '{}' is valid".format(label))
            assert_arg_label_is_fuchsia_package(args.bazel, label)
            labels.append(label)

        if args.dir:
            info("Finding all labels in dir '{}'".format(args.dir))
            labels.extend(collect_labels_from_dir(args))

        if not labels:
            fail("No Bazel labels found from the arguments provided")

    info("Refreshing compdb for Bazel targets: {}".format(labels))

    # TODO(https://fxbug.dev/495869705): Currently it assumes all targets are
    # Fuchsia targets. However, a bazel target may be for host. We should
    # determine the optimize mode for each target individually.
    with open(args.build_dir / "bazel_args" / "fuchsia.json") as f:
        fuchsia_bazel_args = json.load(f)

    bazel_optimize_mode = get_bazel_compilation_mode(fuchsia_bazel_args)

    bazel_config_args = [
        "--compilation_mode={}".format(bazel_optimize_mode),
        "--cpu={}".format(map_bazel_cpu(fuchsia_bazel_args["current_cpu"])),
    ]

    new_compile_commands = bazel_compdb_utils.compdb_for_labels(
        args.build_dir,
        args.bazel,
        bazel_config_args,
        labels,
    )

    compile_commands_path = args.build_dir / "compile_commands.json"
    with open(
        compile_commands_path,
        "r",
    ) as f:
        compile_commands = json.load(f)
        compile_commands.extend(new_compile_commands)

    with open(
        compile_commands_path,
        "w",
    ) as f:
        json.dump(bazel_compdb_utils.dedupe(compile_commands), f, indent=2)

    if args.self_test_filter:
        commands_to_check = [
            c
            for c in compile_commands
            if re.search(args.self_test_filter, c["file"])
        ]
        info("CHECKING {} commands".format(len(commands_to_check)))
        info(
            "SKIPPING {} commands".format(
                len(compile_commands) - len(commands_to_check)
            )
        )
        num_failures = 0

        for command in commands_to_check:
            if "arguments" in command:
                clang_args = command["arguments"]
            else:
                clang_args = command["command"].split()

            try:
                subprocess.check_output(
                    clang_args,
                    text=True,
                    cwd=command["directory"],
                    stderr=None if args.verbose else subprocess.DEVNULL,
                )
            except subprocess.CalledProcessError:
                num_failures += 1

        if num_failures > 0:
            info(f"SELF TEST RESULTS: {num_failures} FAILURES")
            sys.exit(1)
        else:
            info("SELF TEST PASSED WITH NO FAILURES")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
