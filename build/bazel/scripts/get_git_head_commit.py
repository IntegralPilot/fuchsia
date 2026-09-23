#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Extract the git HEAD commit value from a given git source directory.

This also supports creating a Ninja depfile to track the files that
participate in its computation.
"""

import argparse
import subprocess
import sys
from pathlib import Path

# Add build/git to sys.path to import git_utils
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "git"))
from git_utils import find_git_head_inputs as find_git_head_inputs


def get_git_head_commit(repo_dir: Path, git_binary: Path = Path("git")) -> str:
    """Return the git HEAD commit value of a given repository.

    Args:
        repo_dir: Path to a git repository directory.
        git_binary: Optional path to the git binary to use, default to "git".
    Returns:
        An hexadecimal string value for the HEAD commit (even when on a branch).
        This will be "GIT_ERROR" if an error happened when trying to get it.
    """
    ret = subprocess.run(
        [
            git_binary,
            "--no-optional-locks",
            "-C",
            repo_dir,
            "rev-parse",
            "HEAD",
        ],
        text=True,
        capture_output=True,
    )
    if ret.returncode != 0:
        return "GIT_ERROR"

    return ret.stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawTextHelpFormatter
    )
    parser.add_argument(
        "--git",
        type=Path,
        help="Specify path to git binary (auto-detected from PATH)",
    )
    parser.add_argument(
        "--output", type=Path, help="Optional output file path."
    )
    parser.add_argument(
        "--depfile", type=Path, help="Optional Ninja depfile output file path."
    )
    parser.add_argument(
        "repo_dir", type=Path, help="Path to git repository directory."
    )
    args = parser.parse_args()

    if args.depfile and not args.output:
        parser.error("--depfile option requires --output.")

    result = get_git_head_commit(args.repo_dir, args.git)
    if args.output:
        args.output.write_text(result)
    else:
        print(result)

    if args.depfile:
        depfile_inputs = find_git_head_inputs(args.repo_dir)
        args.depfile.write_text(
            "%s: %s\n"
            % (args.output, " ".join(str(f) for f in sorted(depfile_inputs)))
        )

    return 0


if __name__ == "__main__":
    sys.exit(main())
