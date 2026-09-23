#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import re
import sys
from pathlib import Path

# Keywords that are not targets, or are not targets that we care about.
GN_IGNORED_KEYWORDS = {
    "gn": {
        "assert",
        "config",
        "declare_args",
        "defined",
        "else",
        "exec_script",
        "filter_exclude",
        "filter_include",
        "filter_labels_exclude",
        "filter_labels_include",
        "foreach",
        "forward_variables_from",
        "get_label_info",
        "get_path_info",
        "get_target_outputs",
        "getenv",
        "group",
        "if",
        "import",
        "label_matches",
        "len",
        "not_needed",
        "path_exists",
        "pool",
        "print",
        "print_stack_trace",
        "process_file_template",
        "read_file",
        "rebase_path",
        "set_default_toolchain",
        "set_defaults",
        "split_list",
        "string_join",
        "string_replace",
        "string_split",
        "target",
        "template",
        "tool",
        "toolchain",
        "write_file",
    },
    "bazel": {
        "export_files",
        "load",
        "package",
    },
}


def count_targets(
    fuchsia_root: Path,
    skip_dirs: set[str],
    exclude_targets: set[str],
    system: str,
) -> dict[str, int]:
    build_file = f"BUILD.{system}"
    print(f"Scanning {build_file} files (skipping: {', '.join(skip_dirs)})...")

    # We look for an identifier followed by an open parenthesis.
    # Irrelevant keywords are filtered out later.
    target_pattern = re.compile(r"^\s*([a-zA-Z0-9_]+)\s*\(")

    # Keywords to ignore (e.g. GN built-in functions that are not targets).
    ignored_keywords = GN_IGNORED_KEYWORDS[system] | exclude_targets

    counts = {}
    for file_path in fuchsia_root.rglob(build_file):
        relative_path = file_path.relative_to(fuchsia_root)
        parts = relative_path.parts
        if any(part in skip_dirs for part in parts):
            continue

        with open(file_path, "r") as f:
            for line in f:
                match = target_pattern.match(line)
                if not match:
                    continue
                name = match.group(1)
                if name not in ignored_keywords:
                    counts[name] = counts.get(name, 0) + 1
    return counts


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Count target types and usage for specified build system."
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=Path,
        default=Path(os.environ.get("FUCHSIA_DIR")),
        help="Path to Fuchsia root directory",
    )
    parser.add_argument(
        "--system",
        type=str,
        required=True,
        choices=["gn", "bazel"],
        help="Build system to count targets for, valid options are [gn, bazel]",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=20,
        help="Number of top target types to show, reverse ordered by counts",
    )
    parser.add_argument(
        "--skip",
        action="append",
        type=str,
        default=["out", "vendor", "third_party"],
        help="Directories to skip (can be specified multiple times)",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        type=str,
        default=[],
        help="Target types to exclude (can be specified multiple times)",
    )
    args = parser.parse_args()

    target_counts = count_targets(
        args.fuchsia_dir, set(args.skip), set(args.exclude), args.system
    )
    if target_counts:
        print(
            f"\n======== {args.system.upper()} Target and Template Usages =========\n"
        )
        print(f"{'Target type':<40} {'Count':<8}")
        print("-" * 48)
        sorted_targets = sorted(
            target_counts.items(), key=lambda x: x[1], reverse=True
        )
        for name, count in sorted_targets[: args.top]:
            print(f"{name:<40} {count:<8}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
