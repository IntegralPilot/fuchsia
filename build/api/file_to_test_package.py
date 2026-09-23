# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import shlex
import sys
import typing as T
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../bazel/scripts"))
import build_utils

if T.TYPE_CHECKING:
    from gn_ninja_outputs import NinjaOutputsBase


class FileToTestPackageFinder:
    """Finds test packages that cover a specific source file.

    This class encapsulates the logic for "fast path" lookups using
    build artifacts like rust-project.json and compile_commands.json,
    avoiding the slower full-graph analysis when possible.

    Args:
        build_dir: The root of the build directory.
        fuchsia_dir: The root of the Fuchsia source tree.
        outputs_database: An instance of NinjaOutputsBase for querying build outputs.
        log_func: A callable for logging messages.
        command_runner: An instance of build_utils.CommandRunner.
        host_tag: The host platform tag (e.g. "linux-x64").
    """

    def __init__(
        self,
        build_dir: Path,
        fuchsia_dir: Path,
        outputs_database: "NinjaOutputsBase",
        log_func: T.Callable[[str], None],
        host_tag: str = "linux-x64",
        command_runner: build_utils.CommandRunner | None = None,
    ):
        self.build_dir = build_dir
        self.fuchsia_dir = fuchsia_dir
        self.outputs_database = outputs_database
        self.log_func = log_func
        self.host_tag = host_tag
        self._command_runner = command_runner or build_utils.CommandRunner(
            self.log_func
        )

    def find_test_packages_fast(self, source_path: Path | str) -> set[str]:
        """Finds test packages using fast-lookup heuristics.

        Args:
            source_path: The relative path to the source file (e.g., "src/lib.rs").

        Returns:
            A set of GN labels for test packages that likely cover this source file.
        """
        # Helper to get GN labels for the source file
        gn_labels: set[str] = set()

        str_abs_source = os.path.realpath(self.fuchsia_dir / source_path)

        # 1. rust-project.json (for Rust)
        rust_project_path = self.build_dir / "rust-project.json"
        if rust_project_path.exists():
            try:
                with open(rust_project_path) as f:
                    rust_data = json.load(f)

                test_labels = set()
                other_labels = set()

                # Schema for the input file is at
                # https://rust-analyzer.github.io/book/non_cargo_based_projects.html
                # but is roughly summarized as:
                # {
                #   "crates": [
                #     {
                #       "root_module": "/path/to/root.rs",
                #       "label": "//path/to:crate",
                #       "source": {
                #         "include_dirs": ["/path/to/include"],
                #       },
                #       "cfg": ["feature=default", "test"],
                #     },
                #   ],
                # }

                for crate in rust_data.get("crates", []):
                    # Check if this crate matches our source file
                    is_match = False
                    root_module = crate.get("root_module", "")
                    if root_module == str_abs_source:
                        is_match = True
                    else:
                        # Check include_dirs
                        source_opts = crate.get("source", {})
                        include_dirs = source_opts.get("include_dirs", [])
                        for d in include_dirs:
                            if str_abs_source.startswith(d):
                                is_match = True
                                break

                    if is_match:
                        label = crate.get("label", "")
                        if label:
                            # Check if this is a test crate
                            cfg = crate.get("cfg", [])
                            if "test" in cfg:
                                test_labels.add(label)
                            else:
                                other_labels.add(label)

                # Prefer test labels if found
                if test_labels:
                    gn_labels = test_labels
                else:
                    gn_labels = other_labels

            except (ValueError, OSError, json.JSONDecodeError) as e:
                self.log_func(f"rust-project.json search failed: {e}")

        # 2. compile_commands.json (for C++)
        if not gn_labels:
            cc_path = self.build_dir / "compile_commands.json"
            if cc_path.exists():
                with open(cc_path) as f:
                    cc_data = json.load(f)

                # Prepare relative path for matching
                # compile_commands.json usually uses paths relative to build_dir
                rel_source = os.path.relpath(str_abs_source, self.build_dir)

                for entry in cc_data:
                    file_path = entry.get("file", "")

                    if file_path == rel_source or file_path == str_abs_source:
                        match = True
                    elif not os.path.isabs(file_path):
                        # Resolve relative path from build_dir to see if it matches absolute source
                        candidate_abs = os.path.realpath(
                            self.build_dir / file_path
                        )
                        if candidate_abs == str_abs_source:
                            match = True
                        else:
                            match = False
                    else:
                        match = False

                    if match:
                        output = entry.get("output", "")
                        if not output:
                            # Try to extract from command
                            command = entry.get("command", "")
                            try:
                                args = shlex.split(command)
                                # Search backwards for the last -o flag
                                for i in range(len(args) - 1, -1, -1):
                                    arg = args[i]
                                    if arg == "-o":
                                        if i + 1 < len(args):
                                            output = args[i + 1]
                                            break
                                    elif arg.startswith("-o"):
                                        # Handle -oFILENAME
                                        output = arg[2:]
                                        break
                            except (ValueError, OSError) as e:
                                self.log_func(
                                    f"Error splitting command '{command}': {e}"
                                )

                        if output:
                            label = self.outputs_database.path_to_gn_label(
                                output
                            )
                            if label:
                                gn_labels.add(label)
                                break

        if not gn_labels:
            return set()

        # If tests.json is missing, we cannot definitively find test packages.
        # However, if we found any related GN labels, return them as a fallback.
        tests_json_path = self.build_dir / "tests.json"
        if not tests_json_path.exists():
            self.log_func(
                f"WARNING: {tests_json_path} not found. Returning all found GN labels as potential test packages."
            )
            return gn_labels

        # Check for corrupted tests.json upfront to provide useful error/fallback
        # We load it once here and pass the data to _filter_tests_json to avoid double-loading.
        try:
            with open(tests_json_path) as f:
                tests_data = json.load(f)
        except json.JSONDecodeError:
            self.log_func(f"ERROR: Failed to parse corrupted {tests_json_path}")
            return gn_labels
        except (ValueError, OSError) as e:
            self.log_func(f"ERROR: Failed to read {tests_json_path}: {e}")
            return gn_labels

        # Load caching infrastructure
        # This stores a { target_label -> [ test_label ] } map.
        cache: dict[str, list[str]] = self._load_cache()

        final_tests = set()

        for target in gn_labels:
            # Check cache
            if target in cache:
                final_tests.update(cache[target])
                continue

            # Run gn refs --all
            refs = self._run_gn_refs(target)

            # Intersect with tests.json
            matched_tests = self._filter_tests_json(refs, tests_data)

            # Heuristic: Prefer tests defined in the same directory as the source target
            # If we find any such tests, return ONLY them.
            if matched_tests:
                target_dirs: set[str] = set()
                for t in gn_labels:
                    # //foo/bar:baz -> //foo/bar
                    # //foo/bar -> //foo/bar
                    target_dirs.add(t.split(":")[0])

                same_dir_tests: set[str] = set()
                for test in matched_tests:
                    for d in target_dirs:
                        # Match //foo/bar:test or //foo/bar/sub:test
                        if test.startswith(d + ":") or test.startswith(d + "/"):
                            same_dir_tests.add(test)

                if same_dir_tests:
                    matched_tests = same_dir_tests

            # Update cache and result
            cache[target] = list(matched_tests)
            final_tests.update(matched_tests)

        self._save_cache(cache)
        return final_tests

    def _run_gn_refs(self, target: str) -> set[str]:
        """Runs `gn refs` to find all targets depending on `target`."""
        gn_path = (
            self.fuchsia_dir
            / "prebuilt"
            / "third_party"
            / "gn"
            / self.host_tag
            / "gn"
        )
        cmd: list[str | Path] = [
            gn_path,
            "refs",
            self.build_dir,
            target,
            "--all",
        ]
        result = self._command_runner.run_command(
            cmd, capture_output=True, check=False
        )
        if result.returncode == 0:
            return set(
                line.strip()
                for line in result.stdout.splitlines()
                if line.strip()
            )

        self.log_func(f"WARNING: gn refs failed for {target}")
        return set()

    def _filter_tests_json(
        self, refs: set[str], tests_data: list[dict[str, T.Any]]
    ) -> set[str]:
        """Returns test labels from tests.json that are in `refs`."""
        matched = set()
        for test_entry in tests_data:
            test = test_entry.get("test", {})
            label = test.get("label", "")
            # Try removing toolchain
            label_no_toolchain = label.split("(")[0]

            if (label and label in refs) or (
                label_no_toolchain and label_no_toolchain in refs
            ):
                matched.add(label)
                continue

            package_label = test.get("package_label", "")
            package_label_no_toolchain = package_label.split("(")[0]

            if (package_label and package_label in refs) or (
                package_label_no_toolchain
                and package_label_no_toolchain in refs
            ):
                matched.add(label)

        return matched

    def _get_cache_path(self) -> Path:
        return self.build_dir / "file_to_test_package_cache.json"

    def _load_cache(self) -> dict[str, list[str]]:
        """Loads cache if valid, else returns empty dict."""
        cache_path = self._get_cache_path()
        if not cache_path.exists():
            return {}

        try:
            with open(cache_path) as f:
                data = json.load(f)
        except (ValueError, OSError):
            return {}

        # Check dependencies
        deps = [
            "tests.json",
            "rust-project.json",
            "compile_commands.json",
            "args.gn",
        ]

        try:
            cache_mtime_ns = cache_path.stat().st_mtime_ns
        except FileNotFoundError:
            return {}

        for dep in deps:
            dep_path = self.build_dir / dep
            if dep_path.exists():
                try:
                    dep_mtime_ns = dep_path.stat().st_mtime_ns
                    if dep_mtime_ns > cache_mtime_ns:
                        return {}  # Stale
                except OSError:
                    self.log_func(f"WARNING: Could not stat {dep_path}")

        return data.get("mapping", {})

    def _save_cache(self, mapping: dict[str, list[str]]) -> None:
        """Saves the mapping to cache with current timestamp."""
        cache_path = self._get_cache_path()
        data = {"mapping": mapping}
        try:
            with open(cache_path, "w") as f:
                json.dump(data, f)
        except (ValueError, OSError) as e:
            self.log_func(f"WARNING: Failed to save cache: {e}")
