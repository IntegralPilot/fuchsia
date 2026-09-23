# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
from pathlib import Path


def get_git_path(repo_dir: Path, name: str) -> Path:
    """Return the absolute, resolved path to a git metadata file.

    Args:
        repo_dir: Path to the repository root directory.
        name: Name of the git path to resolve (e.g. 'HEAD', 'index').
    Returns:
        The absolute path as a Path object.
    """
    # Avoid passing `--path-format=absolute` here. In repositories using the
    # experimental `reftable` backend (default in newer Git versions with
    # experimental features enabled), loose reference files like refs/heads/main
    # do not physically exist on disk, causing `git rev-parse --path-format=absolute`
    # to fail with a fatal error. Resolving the relative path in Python avoids this.
    ret = subprocess.run(
        [
            "git",
            "--no-optional-locks",
            "-C",
            str(repo_dir),
            "rev-parse",
            "--git-path",
            name,
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    git_path = Path(ret.stdout.strip())
    if not git_path.is_absolute():
        git_path = repo_dir / git_path
    return git_path.resolve()


def get_git_ref(repo_dir: Path) -> Path:
    """Return the absolute path to the current branch ref file or packed-refs.

    If the repository is in detached HEAD state, returns the path to the HEAD file.
    If the current branch ref is loose, returns the path to the loose ref file.
    If the current branch ref is packed, returns the path to the packed-refs file.

    Args:
        repo_dir: Path to the repository root directory.
    Returns:
        The absolute path as a Path object.
    """
    # Find the current ref if on a branch
    cmd = [
        "git",
        "--no-optional-locks",
        "-C",
        str(repo_dir),
        "symbolic-ref",
        "HEAD",
    ]
    ret = subprocess.run(cmd, capture_output=True, text=True)
    if ret.returncode != 0:
        # Detached HEAD. We track HEAD.
        return get_git_path(repo_dir, "HEAD")

    ref_name = ret.stdout.strip()
    ref_path = get_git_path(repo_dir, ref_name)
    if ref_path.exists():
        return ref_path

    # If the ref is packed, it won't exist as a loose file.
    # We track packed-refs instead.
    packed_refs = get_git_path(repo_dir, "packed-refs")
    if packed_refs.exists():
        return packed_refs

    # Fallback to HEAD if ref/packed-refs don't exist for some reason
    return get_git_path(repo_dir, "HEAD")


def find_git_head_inputs(
    repo_dir: Path, track_index: bool = False
) -> set[Path]:
    """Find all input files that affect the git HEAD of a given repository.

    Args:
        repo_dir: Path to a non-bare git repository directory.
        track_index: If True, also track the index file (useful for dirty checks).
    Returns:
        A set of Path values that can be used as implicit inputs in a depfile.
    """
    result: set[Path] = set()

    git_file = repo_dir / ".git"
    if git_file.is_file():
        result.add(git_file)

    git_head = get_git_path(repo_dir, "HEAD")
    result.add(git_head)

    git_config = get_git_path(repo_dir, "config")
    if git_config.exists():
        result.add(git_config)

    git_packed_refs = get_git_path(repo_dir, "packed-refs")
    if git_packed_refs.exists():
        result.add(git_packed_refs)

    # Track the ref file (or packed-refs/HEAD fallback)
    ref_path = get_git_ref(repo_dir)
    result.add(ref_path)

    # Track reftable metadata files if they exist (used by the reftable backend)
    git_reftable = get_git_path(repo_dir, "reftable")
    if git_reftable.exists():
        tables_list = git_reftable / "tables.list"
        if tables_list.exists():
            result.add(tables_list)
        else:
            result.add(git_reftable)

    if track_index:
        git_index = get_git_path(repo_dir, "index")
        if git_index.exists():
            result.add(git_index)

    return result


def _get_best_ref(project_path: Path) -> str:
    """Determine the best git ref to use for resolving the revision.

    Checks candidates like 'HEAD', 'jiri/head', 'origin/main' in order
    and returns the first one that exists.

    Args:
        project_path: Path to the git repository.
    Returns:
        The ref name to use (e.g., 'HEAD').
    """
    candidates = ["HEAD", "jiri/head", "origin/main"]
    for c in candidates:
        ret = subprocess.run(
            [
                "git",
                "--no-optional-locks",
                "-C",
                str(project_path),
                "rev-parse",
                "--verify",
                c,
            ],
            capture_output=True,
            text=True,
        )
        if ret.returncode == 0:
            return c
    return "HEAD"


def get_git_revision(project_path: Path) -> str:
    """Return the git revision hash for the project.

    Automatically resolves the best ref to use (e.g. HEAD).

    Args:
        project_path: Path to the git repository.
    Returns:
        The 40-character hexadecimal revision hash.
    """
    ref = _get_best_ref(project_path)
    ret = subprocess.run(
        [
            "git",
            "--no-optional-locks",
            "-C",
            str(project_path),
            "rev-parse",
            ref,
        ],
        text=True,
        capture_output=True,
        check=True,
    )
    return ret.stdout.strip()
