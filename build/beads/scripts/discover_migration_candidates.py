#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import csv
import dataclasses
import pathlib
import re
import sys

import gn_runner

# Debug flag to enable verbose output.
_DEBUG = False

# The root directory of the Fuchsia source tree.
_FUCHSIA_DIR = pathlib.Path(__file__).parent.parent.parent.parent

# Fields that are considered dependencies. Dependencies need to be migrated to Bazel first before
# this target can be migrated.
_DEP_FIELDS = [
    "deps",
    "public_deps",
    "test_deps",
    "data",
    "embed",
    "plugin_deps",
    "args_deps",
    "args_test_deps",
    "ffx_deps",
    "proc_macro_deps",
]

# Fields that are considered "standard" and easy to convert.
_STANDARD_FIELDS = set(_DEP_FIELDS) | {
    "edition",
    "inputs",
    "name",
    "output_name",
    "source_root",
    "sources",
    "testonly",
    "visibility",
    "with_unit_tests",
}

# Default complexity scores for non-simple deps.
_DEFAULT_DEP_COMPLEXITY = 1

# Complexity score for deps not directly defined in the input GN files (they can be defined in other
# GN files, or in gni files as subtargets).
_UNKNOWN_DEP_COMPLEXITY = 5

# Default complexity scores for non-standard fields.
_DEFAULT_FIELD_COMPLEXITY = 1

# Fields that are considered "complex" and require more attention.
#
# This dictionary maps field names to their complexity score.
_COMPLEX_FIELDS = {
    "configs": 2,
    "rustc_flags": 2,
    "forward_variables_from": 2,
}

# Mapping from GN template type to a list of subtarget suffixes it generates.
# This is used to resolve generated implicit subtargets back to their base target.
_TEMPLATE_SUBTARGET_SUFFIXES = {
    "ffx_plugin": ["_suite", "_args", "_sub_command", "_test", "_tests"],
    "ffx_tool": [
        "_versioned",
        "_unversioned",
        "_host_tool",
        "_test",
        "_tests",
        "_bin",
        "_bin_unversioned",
        "_bin_unversioned_test",
    ],
}

# Toolchain shorthands for convenience.
_TOOLCHAIN_SHORTHANDS = {
    "host": "//build/toolchain:host_x64",
    "fuchsia_arm64": "//build/toolchain/fuchsia:arm64",
    "fuchsia_x64": "//build/toolchain/fuchsia:x64",
}


def debug(msg: str) -> None:
    if _DEBUG:
        print(msg, file=sys.stderr)


def find_gn_files(
    start_dir: pathlib.Path,
    exclude_dirs: list[str],
    toolchain: str | None = None,
) -> list[pathlib.Path]:
    """Find all BUILD.gn files in the specified directory and its subdirectories.

    Args:
        start_dir: The directory to search for BUILD.gn files.
        exclude_dirs: A list of directories to exclude from the search.
        toolchain: If provided, only include files that have targets with this
                   toolchain.

    Returns:
        A list of BUILD.gn file paths.
    """
    gn = gn_runner.GnRunner()

    if toolchain:
        toolchain = _TOOLCHAIN_SHORTHANDS.get(toolchain, toolchain)
        gn_args = ["ls", f"//{start_dir}/*({toolchain})", "--as=buildfile"]
    else:
        gn_args = [
            "ls",
            f"//{start_dir}/*",
            "--all-toolchains",
            "--as=buildfile",
        ]

    output = gn.run_and_extract_output(gn_args)
    all_buildfiles = [
        pathlib.Path(line).relative_to(_FUCHSIA_DIR)
        for line in output.splitlines()
    ]
    return [
        buildfile
        for buildfile in all_buildfiles
        if not any(
            buildfile.is_relative_to(excluded_dir)
            for excluded_dir in exclude_dirs
        )
    ]


def end_pos_for_target(content: str, start_pos: int) -> int:
    """Find the matching closing brace for a target."""
    brace_count = 1
    end_pos = start_pos

    while brace_count > 0 and end_pos < len(content):
        if content[end_pos] == "{":
            brace_count += 1
        elif content[end_pos] == "}":
            brace_count -= 1
        end_pos += 1

    if brace_count != 0:
        raise ValueError("Unmatched braces in BUILD.gn file")

    return end_pos


def deps_from_target_body(
    target_body: str, context: dict[str, list[str]]
) -> list[str]:
    """Extract deps from a GN target body.

    Args:
        target_body: The body of the GN target.
        context: The context containing shared variables.

    Returns:
        A list of dependencies.
    """
    assignments = list_assignments_from(target_body, context)

    final_deps = []
    for var in _DEP_FIELDS:
        if var in assignments:
            final_deps.extend(assignments[var])

    return final_deps


def fields_from_target_body(target_body: str) -> list[str]:
    """Extract fields from a GN target body."""
    fields = set()
    for field_match in re.finditer(
        r"^\s*([a-z_]+)\s*\+?=",
        target_body,
        re.MULTILINE,
    ):
        fields.add(field_match.group(1))
    return list(fields)


def shared_variables_from(context: str) -> dict[str, list[str]]:
    """Extract shared variables from a string."""
    return list_assignments_from(context, {})


def list_assignments_from(
    content: str, context: dict[str, list[str]]
) -> dict[str, list[str]]:
    """Extract list assignments from a string.

    This function implements a simple regex-based parser to extract variable
    assignments from a string. It also resolves variable references using the
    provided context. The assignments are expected to be of the form
    "var = [list of strings]" or "var += [list of strings]".

    NOTE: It is NOT a goal of this function and this script to handle all
    valid BUILD.gn syntaxes. It prefers simplicity over completeness.

    Args:
        content: The string to parse.
        context: A dictionary to resolve variable references.

    Returns:
        A dictionary of variable assignments, where the keys are variable names
        and the values are lists of strings.
    """

    # Remove comments to avoid accidentally matching variable names in comments.
    text = re.sub(r"#.*$", "", content, flags=re.MULTILINE)

    assignments: dict[str, list[str]] = {}

    # Regex to find assignments: var = ... or var += ...
    # Captures the variable name and the operator.
    # We allow the lookahead to match the start of the next assignment or
    # end of string.
    assign_pattern = re.compile(r"(?:^|[\s;}])(\w+)\s*(\+?=)")

    matches = list(assign_pattern.finditer(text))

    for i, match in enumerate(matches):
        lhs = match.group(1)
        op = match.group(2)

        # The RHS is the text from the end of this match to the start of the
        # next match (or end of text).
        start_rhs = match.end()
        end_rhs = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        rhs = text[start_rhs:end_rhs]

        # Extract string literals from the RHS.
        rhs_values = re.findall(r'"([^"]*)"', rhs)

        # Remove string literals from RHS in order to find variable references.
        rhs_no_strings = re.sub(r'"[^"]*"', "", rhs)

        # Find variable references in the RHS.
        refs = re.findall(r"\b(\w+)\b", rhs_no_strings)
        for ref in refs:
            # Skip variables not known to the context.
            if ref in context:
                rhs_values.extend(context[ref])

        if op == "+=":
            assignments.setdefault(lhs, []).extend(rhs_values)
        else:
            assignments[lhs] = rhs_values

    return assignments


@dataclasses.dataclass
class GnTargetInfo:
    """Information about a target definition found in a BUILD.gn file."""

    name: str
    type: str
    path: pathlib.Path
    deps: list[str] = dataclasses.field(default_factory=list)
    fields: list[str] = dataclasses.field(default_factory=list)
    complexity: int = 0
    non_standard_fields: list[str] = dataclasses.field(default_factory=list)


@dataclasses.dataclass
class GnFileInfo:
    """Information about a single BUILD.gn fle."""

    path: pathlib.Path
    targets: list[GnTargetInfo] = dataclasses.field(default_factory=list)
    total_complexity: int = 0
    total_targets: int = 0


def targets_from_gn_file(
    filepath: pathlib.Path, target_types: list[str] | None
) -> list[GnTargetInfo]:
    """Parse a BUILD.gn file and extract information about targets in input target_types.

    This function implements a simple regex-based parser for BUILD.gn files.
    It looks for `target_type(name) { ... }` patterns and extracts dependencies
    and fields from the target body.

    It also accumulates context, which include variable assignments outside of
    target bodies. Context can be used to resolve shared variable references in
    target bodies.

    Args:
        filepath: Path to the BUILD.gn file.
        target_types: List of target types to look for. None means all target types.

    Returns:
        List of dictionaries, each one describing a target found in the file.
    """
    full_path = _FUCHSIA_DIR / filepath
    if not full_path.exists():
        secondary_path = _FUCHSIA_DIR / "build" / "secondary" / filepath
        if secondary_path.exists():
            full_path = secondary_path
        else:
            print(f"WARNING: File not found: {filepath}", file=sys.stderr)
            return []

    content = full_path.read_text()

    # This regex looks for any of the input target_types followed by a name
    # and an opening brace.
    target_re = re.compile(
        r'(\w+)\(*"([^"]+)"\)\s*\{',
    )

    context = ""
    targets = []
    pre_end = 0
    for match in target_re.finditer(content):
        # Accumulate context, which include variable assignments outside of
        # target bodies. Context can be used to resolve shared variable
        # references in target bodies.
        context += content[pre_end : match.start()]

        start_pos = match.end()
        try:
            end_pos = end_pos_for_target(content, start_pos)
            # Record the end position of the target body for context accumulation.
            pre_end = end_pos
        except ValueError as e:
            print(
                f"WARNING: Error parsing {filepath} after position {start_pos}: {e}",
                file=sys.stderr,
            )
            continue

        target_type, target_name = match.group(1), match.group(2)
        # Skip targets that are not in the target_types we care about.
        # It's still necessary to do previous parsing to extract shared variables.
        if target_types and target_type not in target_types:
            continue

        target_body = content[start_pos : end_pos - 1]
        targets.append(
            GnTargetInfo(
                type=target_type,
                name=target_name,
                path=filepath,
                deps=deps_from_target_body(
                    target_body, shared_variables_from(context)
                ),
                fields=fields_from_target_body(target_body),
            )
        )

    return targets


class ComplexityCalculator:
    def __init__(
        self,
        root_dir: pathlib.Path,
        gn_files: list[pathlib.Path],
        target_types: list[str],
    ):
        """Initialize the complexity calculator.

        Args:
            root_dir: The root directory of the codebase.
            gn_files: List of GN files to parse.
            target_types: List of target types to extract.
        """
        self._root_dir = root_dir.resolve()
        # Cache for computed complexities keyed on fully-qualified labels.
        self._complexity_cache: dict[str, int] = {}
        # Cache for target details keyed on fully-qualified labels.
        self._target_cache: dict[str, GnTargetInfo] = {}
        # Populate the target cache with all targets eagerly in the specified GN files for easier
        # dependency label resolution during complexity calculation, which traverses the dependency
        # graph recursively.
        self._populate_target_cache(gn_files, target_types)

    def _populate_target_cache(
        self, gn_files: list[pathlib.Path], target_types: list[str]
    ) -> None:
        """Populate the target cache with all targets in the specified GN files.

        Args:
            gn_files: List of GN files to parse.
            target_types: List of target types to extract.
        """
        for gn_file in gn_files:
            # Don't filter by target_types here because we need to extract all
            # possible dependencies for each target defined in the GN file.
            # For example, caller could be filtering with `rustc_binary` but the
            # target could have `rustc_library` dependencies.
            targets = targets_from_gn_file(gn_file, None)
            label_to_target = {
                self._to_fully_qualified_label(
                    gn_file.parent, target.name
                ): target
                for target in targets
            }
            self._target_cache.update(label_to_target)

    def _is_third_party_target(self, label: str) -> bool:
        """Check if a target is a third-party target.

        We have generated Bazel targets for third-party Rust and Go libraries.
        These dependencies are simple and can be easily mapped to Bazel.
        """
        if not self._is_fully_qualified_label(label):
            raise ValueError(
                f"Invalid label: {label}, only fully-qualified labels are supported"
            )

        return label.startswith(
            "//third_party/rust_crates"
        ) or label.startswith("//third_party/golibs")

    def _is_bazel_target(self, label: str) -> bool:
        """Check if a target is already in a BUILD.bazel file in the same directory."""
        if not self._is_fully_qualified_label(label):
            raise ValueError(
                f"Invalid label: {label}, only fully-qualified labels are supported"
            )

        path_part, target_name = label.split(":", 1)
        target_name = target_name.split("(")[0]  # Drop toolchain, if any.
        path_dir = self._root_dir / path_part.lstrip("/")
        bazel_file = path_dir / "BUILD.bazel"
        if not bazel_file.exists():
            return False

        content = bazel_file.read_text()
        return (
            f'name = "{target_name}"' in content
            or f'name="{target_name}"' in content
        )

    def _is_fully_qualified_label(self, label: str) -> bool:
        return label.startswith("//") and ":" in label

    def _to_fully_qualified_label(
        self, dir_path: pathlib.Path, label: str
    ) -> str:
        """Convert a label to a fully-qualified label.

        If the label is already fully qualified, return it as is.
        Otherwise, convert it to a fully-qualified label.
        """
        if self._is_fully_qualified_label(label):
            return label

        if ":" in label:
            label_path, target_name = label.split(":", 1)
            target_path = dir_path / label_path
        elif label.startswith("//"):
            # Shorthand notation like "//src/lib/fdomain/client" -> target is "client"
            label_path = label.lstrip("//")
            target_name = pathlib.Path(label_path).name
            target_path = pathlib.Path(label_path)
        else:
            target_name = label
            target_path = dir_path
        return f"//{target_path}:{target_name}"

    def _parts_from_fully_qualified_label(
        self, label: str
    ) -> tuple[pathlib.Path, str]:
        """Return the path and target name from a fully-qualified label."""
        if not self._is_fully_qualified_label(label):
            raise ValueError(
                f"Invalid label: {label}, only fully-qualified labels are supported"
            )
        path_part, target_name = label.lstrip("//").split(":", 1)
        return pathlib.Path(path_part), target_name

    def _resolve_template_subtarget(self, label: str) -> str | None:
        """Resolve a generated template subtarget to its base label.

        Args:
            label: The fully qualified label of the subtarget.

        Returns:
            The fully qualified base label if found, otherwise None.
        """
        parts = label.split(":", 1)
        if len(parts) != 2:
            return None

        base_path, target_name = parts

        for target_type, suffixes in _TEMPLATE_SUBTARGET_SUFFIXES.items():
            # Match the longest ones first.
            for suffix in sorted(suffixes, key=len, reverse=True):
                if not target_name.endswith(suffix):
                    continue
                base_name = target_name[: -len(suffix)]
                base_label = f"{base_path}:{base_name}"

                if (
                    base_label in self._target_cache
                    and self._target_cache[base_label].type == target_type
                ):
                    return base_label
        return None

    def complexity_for_label(
        self, label: str, visiting: set[str] | None = None
    ) -> int:
        """Calculate complexity for a target taking its dependencies and fields into account.

        This recursively aggregates the complexity of its dependencies, adding a penalty for
        non-standard fields. To handle GN templates creating implicit dependencies (e.g. ffx_plugin, ffx_tool),
        we check if generated suffixes map back to their parent target type to traverse the full graph.

        We also track a `visiting` set to detect and break dependency cycles, omitting back-edges
        to prevent infinite recursion and duplicate counting.
        """
        if visiting is None:
            visiting = set()

        next_visiting = visiting | {label}

        if not self._is_fully_qualified_label(label):
            raise ValueError(
                f"Invalid label: {label}, only fully-qualified labels are supported."
            )

        if label in visiting:
            print(
                f"WARNING: Dependency cycle detected at {label}. Path: {visiting}",
                file=sys.stderr,
            )
            return 0
        # If the target is already in Bazel (e.g. bazel2gn targets) or is a third-party target,
        # its complexity is 0.
        if self._is_bazel_target(label) or self._is_third_party_target(label):
            return 0

        if label in self._complexity_cache:
            return self._complexity_cache[label]

        if label not in self._target_cache:
            base_label = self._resolve_template_subtarget(label)
            if base_label is not None:
                res = self.complexity_for_label(base_label, next_visiting)
                return res
            print(f"WARNING: Unknown {label}, ignoring...", file=sys.stderr)
            return _UNKNOWN_DEP_COMPLEXITY

        target = self._target_cache[label]
        dir_path, _ = self._parts_from_fully_qualified_label(label)
        fully_qualified_deps = [
            self._to_fully_qualified_label(dir_path, dep) for dep in target.deps
        ]
        complex_deps = [
            dep
            for dep in fully_qualified_deps
            if (
                not self._is_bazel_target(dep)
                and not self._is_third_party_target(dep)
            )
        ]
        # Each dependency adds (1 + their complexity) to the total complexity of this target.
        dep_complexity = len(complex_deps) + sum(
            self.complexity_for_label(dep, next_visiting)
            for dep in complex_deps
        )

        field_complexity = sum(
            _COMPLEX_FIELDS.get(f, _DEFAULT_FIELD_COMPLEXITY)
            for f in target.fields
            if f not in _STANDARD_FIELDS
        )

        # Add 1 as base complexity for the target itself. To be distinguished
        # from targets with 0 complexity (e.g. bazel2gn targets).
        complexity = 1 + dep_complexity + field_complexity
        self._complexity_cache[label] = complexity
        return complexity

    def complexity_for_file(
        self, gn_file: pathlib.Path, target_types: list[str]
    ) -> GnFileInfo:
        targets = targets_from_gn_file(gn_file, target_types)

        file_result = GnFileInfo(path=gn_file)

        for target in targets:
            complexity = self.complexity_for_label(
                self._to_fully_qualified_label(gn_file.parent, target.name)
            )

            target_info = dataclasses.replace(
                target,
                complexity=complexity,
                # Add extra info for display if needed.
                non_standard_fields=[
                    f for f in target.fields if f not in _STANDARD_FIELDS
                ],
            )

            file_result.targets.append(target_info)
            file_result.total_complexity += complexity
            file_result.total_targets += 1

        return file_result


def _infer_language(target_type: str) -> str:
    """
    Infers the language of a target based on its type.

    Args:
        target_type: The type of the target.

    Returns:
        The inferred language of the target.
    """
    if target_type == "ffx_plugin":
        return "Rust (ffx plugin)"
    elif target_type == "ffx_tool":
        return "Rust (ffx tool)"
    elif target_type.startswith("rustc_"):
        return "Rust"
    elif target_type.startswith("go_"):
        return "Go"
    elif target_type in (
        "executable",
        "source_set",
        "static_library",
        "shared_library",
        "test",
    ):
        return "C++"
    elif target_type.startswith("python_"):
        return "Python"
    return "Unknown"


def _complexity_to_string(complexity: int) -> str:
    """
    Convert a complexity score to a human-readable string.

    Args:
        complexity: The complexity score.

    Returns:
        A string representing the complexity (LOW, MEDIUM, or HIGH).
    """
    if complexity <= 5:
        return "0 - LOW"
    elif complexity <= 20:
        return "1 - MEDIUM"
    else:
        return "2 - HARD"


def print_results_csv(file_results: list[GnFileInfo], top: int) -> None:
    """
    Print the results of the analysis in a CSV format.

    Args:
        file_results: A list of GnFileInfo objects containing the results of the analysis.
        top: The number of top candidates to print.
    """
    writer = csv.writer(sys.stdout)
    writer.writerow(
        [
            "Bug",
            "Migrated?",
            "Tool Name",
            "GN Build File",
            "Language",
            "Owners",
            "Estimated Complexity",
        ]
    )
    for file_res in file_results[:top]:
        for target in file_res.targets:
            is_migrated = "YES" if target.complexity == 0 else "NO"
            writer.writerow(
                [
                    "",
                    is_migrated,
                    target.name,
                    f"//{file_res.path}",
                    _infer_language(target.type),
                    "",
                    _complexity_to_string(target.complexity),
                ]
            )


def print_results(file_results: list[GnFileInfo], top: int) -> None:
    """Print the results of the analysis in a tabular format."""
    print("\nTop Candidates for Migration (Grouped by File):")
    print("=" * 80)
    for i, file_res in enumerate(file_results[:top]):
        print(f"{i+1}. {file_res.path}")
        print(f"   Targets ({file_res.total_targets}):")
        for target in file_res.targets:
            is_migrated = "YES" if target.complexity == 0 else "NO"
            print(f"     - {target.name} ({target.type})")
            print(f"       Migrated: {is_migrated}")
            print(f"       Inferred language: {_infer_language(target.type)}")
            print(
                f"       Estimated Complexity: {_complexity_to_string(target.complexity)}"
            )
            if target.non_standard_fields:
                print(
                    f"       Non-standard fields: {target.non_standard_fields}"
                )
        print("-" * 80)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="""
        Discover migration candidates for GN targets.

        This script analyzes GN BUILD files in a directory and identifies targets
        that are good candidates for migration to Bazel. It uses a heuristic
        to calculate the complexity of each target based on its dependencies and
        fields, and then ranks them by complexity.

        NOTE: This script only considers BUILD.gn files with targets included in
        your current build configuration (e.g. your `fx set` line).
        """
    )
    parser.add_argument(
        "--directory",
        type=pathlib.Path,
        required=True,
        help="Directory to search for BUILD.gn files",
    )
    parser.add_argument(
        "--exclude-dirs",
        type=str,
        nargs="*",
        default=[],
        help="Directories to exclude when searching for BUILD.gn files",
    )
    parser.add_argument(
        "--target-types",
        type=str,
        nargs="+",
        required=True,
        help="Target types to look for when searching for BUILD.gn files",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        help="Number of top candidates to show",
    )
    parser.add_argument(
        "--format",
        type=str,
        choices=["text", "csv"],
        default="text",
        help="Output format",
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=pathlib.Path,
        default=_FUCHSIA_DIR,
        help="Path to the Fuchsia directory",
    )
    parser.add_argument(
        "--toolchain",
        type=str,
        default=None,
        help=f"""If provided, focus the search on targets with this GN toolchain.
        Use full toolchain label, or one of the shorthands: {', '.join(_TOOLCHAIN_SHORTHANDS.keys())}.

        NOTE: This filters on a per-file basis. A file is included if any of its
        targets are built with the specified toolchain.""",
    )
    parser.add_argument(
        "--include-migrated",
        action="store_true",
        help="Include already migrated targets in the output",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Enable verbose output",
    )
    args = parser.parse_args()

    global _DEBUG
    _DEBUG = args.verbose

    debug(
        f"Searching for GN files in {args.directory}, excluding {args.exclude_dirs}"
    )
    gn_files = find_gn_files(args.directory, args.exclude_dirs, args.toolchain)
    debug(f"Found {len(gn_files)} GN files matching the criteria")

    debug("Calculating complexity for each file...")

    calculator = ComplexityCalculator(
        args.fuchsia_dir, gn_files, args.target_types
    )

    file_results = [
        calculator.complexity_for_file(gn_file, args.target_types)
        for gn_file in gn_files
    ]
    if args.include_migrated:
        actionable_files = file_results
    else:
        actionable_files = [f for f in file_results if f.total_complexity > 0]

    # Sort by total complexity and then by number of targets.
    actionable_files.sort(key=lambda x: (x.total_complexity, x.total_targets))

    if args.format == "csv":
        print_results_csv(actionable_files, args.top)
    else:
        print_results(actionable_files, args.top)

    return 0


if __name__ == "__main__":
    sys.exit(main())
