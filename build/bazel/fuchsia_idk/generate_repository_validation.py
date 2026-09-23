#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""An integration test for generate_repository.py.

This invokes generate_repository.py with a fixed input IDK and fake Ninja
output directory, and compare the result with a fixed golden output IDK
directory.

See the `validation_data/README.md` file for details.
"""

import argparse
import difflib
import filecmp
import os
import shutil
import subprocess
import sys
import tempfile
import typing as T
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent


def _get_files_from(top_dir: Path) -> set[str]:
    """Walk the top_dir directory, and return a set of relative paths to all its files."""
    return {
        os.path.relpath(os.path.join(dirpath, filename), top_dir)
        for dirpath, dirnames, filenames in os.walk(top_dir)
        for filename in filenames
    }


def compare_directories(
    left_dir: Path, right_dir: Path
) -> tuple[T.Sequence[str], T.Sequence[str], T.Sequence[str]]:
    """Compare the content of two directories.

    Args:
        left: Path to first directory.
        right: Path to second directory.
    Returns:
        A 3-tuple whose item correspond to the following lists or
        relative path strings:

        - different_files: files that are present in both directories,
          but whose content differs.
        - left_only_file: files that only appear in the `left` directory.
        - right_only_files: files that only appear in the `right` directory.
    """
    left_files = _get_files_from(left_dir)
    right_files = _get_files_from(right_dir)

    left_only_files = left_files - right_files
    right_only_files = right_files - left_files

    # Find files that are different.
    different_files = set()
    common_files = left_files & right_files
    for file in common_files:
        left = left_dir / file
        right = right_dir / file
        if left.is_symlink() != right.is_symlink():
            different_files.add(file)
            continue

        if left.is_symlink():
            if (
                not right.is_symlink()
                or (right.parent / right.readlink()).resolve()
                != (left.parent / left.readlink()).resolve()
            ):
                different_files.add(file)
            continue

        if not filecmp.cmp(left, right, shallow=False):
            different_files.add(file)
            continue

    return (
        sorted(different_files),
        sorted(left_only_files),
        sorted(right_only_files),
    )


class BuildDir(object):
    """Convenience class to model either a build directory."""

    def __init__(self, build_dir: None | Path = None) -> None:
        """Create new instance.

        Args:
           build_dir: Path to a build directory, or None, in which cases
              a temporary directory will be created, whose content will
              be cleaned up automatically when this instance is freed.
        """
        if build_dir:
            self._top_dir = build_dir
            if self._top_dir.exists():
                shutil.rmtree(self._top_dir, ignore_errors=True)
            self._top_dir.mkdir(parents=True)
        else:
            self._tmp_dir = tempfile.TemporaryDirectory(
                prefix="generate_idk_repository-integration-"
            )
            self._top_dir = Path(self._tmp_dir.name)

    @property
    def path(self) -> Path:
        return self._top_dir


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--validation_data-dir",
        type=Path,
        default=_SCRIPT_DIR / "validation_data",
        help="Specify alternative validation_data/ directory.",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        help="Optional build directory used for development. Useful to see what was generated and compare it with the expected IDK output.",
    )
    parser.add_argument(
        "--stamp-file", type=Path, help="Output stamp file path."
    )
    parser.add_argument(
        "--quiet", action="store_true", help="Do not print anything on success."
    )
    parser.add_argument(
        "--no-diff-check",
        action="store_true",
        default=False,
        help="Disable diff check, for development.",
    )
    parser.add_argument(
        "--hermetic-inputs-file",
        type=Path,
        help="Output file path to write the list of implicit inputs for this script.",
    )

    args = parser.parse_args()

    if args.hermetic_inputs_file:
        # Handle --hermetic_inputs_file here, results must be relative
        # to the current directory.
        implicit_inputs = []
        for root, subdirs, subfiles in os.walk(args.validation_data_dir):
            for subfile in subfiles:
                implicit_inputs.append(
                    os.path.relpath(os.path.join(root, subfile))
                )

        args.hermetic_inputs_file.parent.mkdir(parents=True, exist_ok=True)
        args.hermetic_inputs_file.write_text("\n".join(sorted(implicit_inputs)))
        return 0

    build_dir = BuildDir(args.build_dir)

    # First, copy the source test_data tree to a temporary directory.
    top_dir = build_dir.path
    shutil.copytree(
        args.validation_data_dir, top_dir, symlinks=True, dirs_exist_ok=True
    )

    generate_repository_script = _SCRIPT_DIR / "generate_repository.py"

    # Second, run the script to generate output_idk
    ret = subprocess.run(
        [
            sys.executable,
            "-S",
            str(generate_repository_script),
            "--repository-name",
            "test_idk",
            "--input-dir",
            f"{top_dir}/input_idk",
            "--ninja-build-dir",
            f"{top_dir}/ninja_artifacts",
            "--output-dir",
            f"{top_dir}/output_idk",
        ]
    )
    ret.check_returncode()

    output_idk = top_dir / "output_idk"
    expected_idk = top_dir / "expected_idk"

    if not args.no_diff_check:
        diff_files, left_only, right_only = compare_directories(
            expected_idk, output_idk
        )

        errors: list[str] = []
        if left_only:
            errors.append(
                "Missing files from the output IDK:\n  %s\n"
                % ("\n  ".join(sorted(left_only)))
            )
        if right_only:
            errors.append(
                "Unexpected files in the output IDK:\n  %s\n"
                % ("\n  ".join(sorted(right_only)))
            )
        if diff_files:
            errors.append("Differences between golden and output IDK:")
            for file_name in diff_files:
                left_file = expected_idk / file_name
                right_file = output_idk / file_name
                if not left_file.exists():
                    errors.append(f"< INVALID SYMLINK {left_file}")
                if not right_file.exists():
                    errors.append(f"> INVALID SYMLINK {right_file}")
                if left_file.exists() and right_file.exists():
                    diff_lines = difflib.unified_diff(
                        left_file.read_text().splitlines(),
                        right_file.read_text().splitlines(),
                        f"expected_idk/{file_name}",
                        f"output_idk/{file_name}",
                        lineterm="",
                    )
                    for line in diff_lines:
                        errors.append("   " + line)

        if errors:
            print("ERRORS:\n" + "\n".join(errors), file=sys.stderr)
            return 1

    if args.stamp_file:
        args.stamp_file.write_text("")

    if not args.quiet:
        print("OK")

    return 0


if __name__ == "__main__":
    sys.exit(main())
