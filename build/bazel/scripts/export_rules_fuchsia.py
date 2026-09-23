#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# IMPORTANT: This script should not depend on any Bazel workspace setup
# or specific environment. Only depend on standard Python3 modules
# and assume a minimum of Python3.8 is being used.

"""Generate the final version of @rules_fuchsia for OOT projects.

This script does the following:

- Copy //build/bazel_sdk/bazel_rules_fuchsia to ${output_dir}

- Change all instances of `@fuchsia_rules_common//` to `//fuchsia_rules_common/`
  in the copied Bazel files from the previous step.

- Copy //build/bazel_sdk/fuchsia_rules_common to ${output_dir}/fuchsia_rules_common

- Modify ${output_dir}/MODULE.bazel to remove section between markers
  '# BEGIN_FUCHSIA_IN_TREE_ONLY' and '# END_FUCHSIA_IN_TREE_ONLY'

This should ensure that the resulting output directory is a self-contained version of
@rules_fuchsia that can be used as-is in out-of-tree Bazel projects.
"""

import argparse
import os
import shutil
import sys
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
_BUILD_API_DIR = os.path.join(_SCRIPT_DIR, "../../api")
sys.path.insert(0, _BUILD_API_DIR)


def copy_tree(from_dir: Path, to_dir: Path) -> set[Path]:
    """Copy a directory tree into another one, returns set of inputs."""
    result: set[Path] = set()
    for root, dirnames, filenames in os.walk(from_dir):
        for filename in filenames:
            src_path = Path(root) / filename
            dst_path = to_dir / os.path.relpath(src_path, from_dir)
            dst_path.parent.mkdir(parents=True, exist_ok=True)
            result.add(src_path)
            shutil.copy2(src_path, dst_path)
    return result


def remove_marked_section_lines(
    content: str, start_marker: str, end_marker: str
) -> str:
    """Remove all lines in |content| between those containing |start_marker| and |end_marker|."""
    lines: list[str] = []
    found_omitted_section = False
    in_omitted_section = False
    for line in content.splitlines():
        if in_omitted_section:
            if end_marker in line:
                in_omitted_section = False
        else:
            if start_marker in line:
                in_omitted_section = True
                found_omitted_section = True
            else:
                lines.append(line)

    assert (
        found_omitted_section
    ), f"\n\nMissing [{start_marker}] marker from:\n{content}\n"
    assert (
        not in_omitted_section
    ), f"\n\nMissing [{end_marker}] marker from:\n{content}\n"

    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawTextHelpFormatter
    )
    parser.add_argument(
        "--output-dir", required=True, type=Path, help="Output directory path."
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=Path,
        help="Fuchsia source directory (auto-detected).",
    )
    parser.add_argument(
        "--stamp", type=Path, help="Optional output stamp path."
    )
    parser.add_argument(
        "--depfile", type=Path, help="Ninja output depfile path."
    )
    args = parser.parse_args()

    fuchsia_dir = args.fuchsia_dir
    if not fuchsia_dir:
        fuchsia_dir = Path(_SCRIPT_DIR, "../../..")

    output_dir = args.output_dir.resolve()
    if output_dir.exists():
        shutil.rmtree(output_dir)

    # Step 1: copy bazel_rules_fuchsia to output_dir
    depfile_inputs: set[Path] = copy_tree(
        fuchsia_dir / "build/bazel_sdk/bazel_rules_fuchsia", output_dir
    )

    # Step 2: copy fuchsia_rules_common to output_dir/fuchsia_rules_common
    depfile_inputs |= copy_tree(
        fuchsia_dir / "build/bazel_sdk/fuchsia_rules_common",
        output_dir / "fuchsia_rules_common",
    )

    # Step 3: Substitutue @fuchsia_rules_common// with //fuchsia_rules_common/ in output_dir
    for rootdir, dirnames, filenames in os.walk(output_dir):
        for filename in filenames:
            if filename.endswith(".bzl") or filename.endswith(".bazel"):
                filepath = Path(rootdir) / filename
                content = filepath.read_text()
                if "@fuchsia_rules_common" in content:
                    content = content.replace(
                        "@fuchsia_rules_common//:",
                        "@rules_fuchsia//fuchsia_rules_common:",
                    ).replace(
                        "@fuchsia_rules_common//",
                        "@rules_fuchsia//fuchsia_rules_common/",
                    )
                    filepath.write_text(content)

    # Step 4: modify MODULE.bazel to remove specific segment.
    module_bazel_path = output_dir / "MODULE.bazel"
    module_bazel_path.write_text(
        remove_marked_section_lines(
            module_bazel_path.read_text(),
            "BEGIN_FUCHSIA_IN_TREE_ONLY",
            "END_FUCHSIA_IN_TREE_ONLY",
        )
    )

    if args.depfile:
        assert args.stamp, f"--depfile requires --stamp path."
        args.depfile.parent.mkdir(parents=True, exist_ok=True)
        inputs = sorted([str(p) for p in depfile_inputs])
        args.depfile.write_text(f"{args.stamp}: {' '.join(inputs)}\n")

    if args.stamp:
        args.stamp.parent.mkdir(parents=True, exist_ok=True)
        args.stamp.write_text("")

    return 0


if __name__ == "__main__":
    sys.exit(main())
