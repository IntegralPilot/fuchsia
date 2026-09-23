# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utility functions for working with Bazel test targets."""

import json
import os
import sys
import tempfile
import typing as T
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.append(str(_SCRIPT_DIR))
import build_utils
from build_utils import BazelLauncher, BazelPaths


def generate_tests_json(
    bazel_paths: BazelPaths,
    command_runner: build_utils.CommandRunner | None = None,
    quiet: bool = True,
) -> tuple[list[dict[str, T.Any]], set[Path]]:
    """Generate a tests.json file corresponding to all Bazel test targets

    Args:
        bazel_paths: The BazelPaths object to use for path resolution.
        command_runner: An optional CommandRunner instance.
        quiet: Whether to print status updates.

    Returns:
        A pair of two values which are:

        - A list of dictionaries, describing each Bazel host_test() reachable
          from the root_host_targets, according to the tests.json schema.

        - A set of input paths, whose changes would require a regeneration of
          the tests.json file.
    """
    if not command_runner:
        command_runner = build_utils.CommandRunner()

    bazel_launcher = BazelLauncher(bazel_paths.launcher, runner=command_runner)
    starlark_input = _SCRIPT_DIR / "../starlark/FuchsiaHostTestInfo.cquery"

    # Read the text file enumerating all the Bazel targets listed in
    # `bazel_test_suite` GN targets.
    bazel_test_suites_file = (
        bazel_paths.ninja_build_dir / "bazel_test_suites.txt"
    )

    # LINT.IfChange(bazel_host_tests_debug_symbols_json)
    bazel_host_tests_debug_manifest = (
        bazel_paths.ninja_build_dir / "bazel_host_tests.debug_symbols.json"
    )
    # LINT.ThenChange(//build/api/build_api_filter.py:bazel_host_tests_debug_symbols_json)

    # We must guarantee this file exists on disk even if no Bazel tests are discovered.
    # GN's metadata phase unconditionally outputs a pointer to this file in debug_symbols.json,
    # and LastBuildApiFilter propagates it. Initializing it with an empty structure prevents
    # FileNotFoundError in downstream infra scripts.
    bazel_host_tests_debug_manifest.write_text("[]\n")

    if not bazel_test_suites_file.exists():
        return [], {starlark_input}
    suites = bazel_test_suites_file.read_text().splitlines()
    if not suites:
        # Skip running `bazel cquery` to get the full list of tests if no Bazel
        # test suites are included in the build graph, to save time on regen.
        return [], {starlark_input}

    if not quiet:
        print(
            f"Running Bazel cquery to populate `tests.json` because there are Bazel tests "
            f"({len(suites)} bazel_test_suite{'' if len(suites) == 1 else 's'}) in your GN graph."
        )
    with tempfile.NamedTemporaryFile(mode="w") as query_file:
        query_file.write("tests(set(" + " ".join(suites) + "))")
        query_file.flush()

        ret = bazel_launcher.run_query(
            "cquery",
            [
                "--config=host",
                "--output=starlark",
                f"--starlark:file={starlark_input}",
                f"--query_file={query_file.name}",
            ],
            False,
        )
    if ret.returncode != 0:
        raise RuntimeError(f"Failed to run bazel query: {ret.stderr}")

    def make_execroot_path_relative_to_ninja_build_dir(path: str) -> str:
        """Convert a path relative to the Bazel execroot to a path relative to the Ninja build directory."""
        return os.path.relpath(
            bazel_paths.execroot / path, bazel_paths.ninja_build_dir
        )

    target_cpu = "x64"
    args_json_path = bazel_paths.ninja_build_dir / "args.json"
    if args_json_path.exists():
        args_json = json.loads(args_json_path.read_text())
        if "target_cpu" in args_json:
            target_cpu = args_json["target_cpu"]

    tests_json: list[dict[str, T.Any]] = []
    host_test_debug_manifests: list[dict[str, str]] = []
    targets_missing_test_info: list[str] = []

    # LINT.IfChange(debug_symbols)
    def _debug_manifest_entry(
        label: str,
        cpu: str,
        os_val: str,
        unstripped_binary_execroot_path: str,
    ) -> dict[str, str]:
        """Create a debug manifest entry for a host test.

        This is a simple wrapper function to avoid nested IFTTT.
        """
        return {
            "cpu": cpu,
            "debug": make_execroot_path_relative_to_ninja_build_dir(
                unstripped_binary_execroot_path
            ),
            "label": label,
            "os": os_val.lower(),
        }

    # LINT.ThenChange(//BUILD.gn:debug_symbols)

    for line in ret.stdout.splitlines():
        line = line.strip()
        if not line:
            continue

        # The line is a JSON-encoded object that follows the tests.json schema with
        # the following exceptions:
        #  - The 'bazel_execroot_path' and 'bazel_execroot_runtime_deps_path' fields
        #    are present instead of 'path' and 'runtime_deps_path', and they contain
        #    paths relative to the Bazel execroot instead of the Ninja build directory.
        cquery_test = json.loads(line)

        if cquery_test.get("error") == "missing_fuchsia_host_test_info":
            label = _normalize_label(cquery_test.get("label", "unknown"))
            if label not in targets_missing_test_info:
                targets_missing_test_info.append(label)
            continue

        # LINT.IfChange(cquery_output_schema)
        label = cquery_test["label"]
        cpu_map = {"x86_64": "x64", "aarch64": "arm64"}
        cpu = cpu_map.get(cquery_test["cpu"], cquery_test["cpu"])
        os_val = (
            cquery_test["os"].capitalize() if cquery_test["os"] else "Linux"
        )

        test_spec: dict[str, T.Any] = {
            "environments": [],
            "expects_ssh": False,
            "test": {
                "name": _normalize_label(label),
                "label": label,
                # The source label indicates the location in the tree of the
                # source code. For labels in the main workspace, ensure they
                # start with "//".
                "source_label": _normalize_label(label),
                "path": make_execroot_path_relative_to_ninja_build_dir(
                    cquery_test["launcher_execroot_path"]
                ),
                "runtime_deps": make_execroot_path_relative_to_ninja_build_dir(
                    cquery_test["runtime_deps_json_execroot_path"]
                ),
                "os": cquery_test["os"],
                "cpu": cquery_test["cpu"],
            },
        }

        # Only run host tests in infra on x64, because most host tests are for
        # host tools that never need to run on arm64, so it would be wasteful to
        # run them on arm64.
        # TODO(https://fxbug.dev/542710387): Make this more flexible to support
        # running host tests on arm64 on an opt-in basis.
        if target_cpu == "x64":
            test_spec["environments"].append(
                {"dimensions": {"os": os_val, "cpu": cpu}}
            )

        if cquery_test["list_cases_argument"]:
            assert isinstance(test_spec["test"], dict)  # make mypy happy
            test_spec["test"]["list_cases_argument"] = cquery_test[
                "list_cases_argument"
            ]

        if cquery_test.get("unstripped_binary_execroot_path"):
            host_test_debug_manifests.append(
                _debug_manifest_entry(
                    label,
                    cpu,
                    os_val,
                    cquery_test["unstripped_binary_execroot_path"],
                )
            )

        tests_json.append(test_spec)
        # LINT.ThenChange(//build/bazel/starlark/FuchsiaHostTestInfo.cquery:cquery_output_schema)

    # Write out the top-level debug_symbols.json manifest for all Bazel host tests
    # so GN debug_symbol_manifests metadata on //:bazel_test_suites can reference it.
    bazel_host_tests_debug_manifest.write_text(
        json.dumps(host_test_debug_manifests, indent=2) + "\n"
    )

    if targets_missing_test_info:
        if len(targets_missing_test_info) == 1:
            raise RuntimeError(
                f"Target '{targets_missing_test_info[0]}' included in the bazel_test_suites GN group is a test target "
                f"but does not provide FuchsiaHostTestInfo. "
                f"Wrap it with host_go_test(), host_rustc_test(), host_py_test(), or host_test()."
            )
        else:
            targets_list = "\n".join(
                f"  - {t}" for t in targets_missing_test_info
            )
            raise RuntimeError(
                f"The following targets included in the bazel_test_suites GN group are test targets "
                f"but do not provide FuchsiaHostTestInfo:\n{targets_list}\n"
                f"Wrap them with host_go_test(), host_rustc_test(), host_py_test(), or host_test()."
            )

    return tests_json, {starlark_input}


def _normalize_label(label: str) -> str:
    """Return the given label in its normalized form (never starting with "@@//" or "@//")."""
    for prefix in ("@@//", "@//"):
        if label.startswith(prefix):
            return "//" + label.removeprefix(prefix)
    return label
