#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Shared utility functions for splitting Fuchsia Rust dependency update commits.

Provides helpers for command execution, git repository inspection, crate
discovery and classification (copies vs. moves), applying crate file operations,
and grouping copy/move operations.
"""

import json
import os
import re
import subprocess
import sys
import tomllib
from typing import Any, Dict, Iterable, List, Optional, Set, Tuple


def run(cmd: str, cwd: Optional[str] = None) -> str:
    """Execute a shell command and return its trimmed standard output.

    Args:
        cmd: The shell command string to execute.
        cwd: Optional directory path in which to run the command.

    Returns:
        The stripped stdout string produced by the command.

    Raises:
        subprocess.CalledProcessError: If the command returns a non-zero exit code.
    """
    print(f"Running: {cmd}")
    try:
        result = subprocess.run(
            cmd, shell=True, check=True, capture_output=True, text=True, cwd=cwd
        )
        return result.stdout.strip()
    except subprocess.CalledProcessError as e:
        print(f"Error running command: {cmd}")
        print(f"Stdout: {e.stdout}")
        print(f"Stderr: {e.stderr}")
        raise


def verify_clean_repo(repo_path: str) -> None:
    """Verify that the repository path exists and has no uncommitted changes.

    Args:
        repo_path: Absolute or relative path to the root of the git repository.

    Raises:
        SystemExit: If repo_path is not a directory or working tree is dirty.
    """
    if not os.path.isdir(repo_path):
        print(f"Error: {repo_path} is not a directory")
        sys.exit(1)

    status = run("git status --porcelain", cwd=repo_path)
    if status:
        print("Error: Repository is not clean. Commit or stash changes first.")
        print(status)
        sys.exit(1)


def resolve_commit(repo_path: str, ref: str) -> str:
    """Resolve a git reference (branch, tag, or SHA) to a full commit SHA.

    Args:
        repo_path: Path to the git repository.
        ref: The git reference string (e.g., 'HEAD', 'origin/main').

    Returns:
        The full 40-character commit SHA string.
    """
    return run(f"git rev-parse {ref}", cwd=repo_path)


def get_crates(repo_path: str, ref: str) -> Set[Tuple[str, str]]:
    """Discover vendored third-party crates present in Cargo.lock at a given git commit reference.

    Parses third_party/rust_crates/Cargo.lock using tomllib so that crates with
    pre-release or non-canonical version schemes (e.g. 'rc3', 'beta14') are reliably identified.

    Args:
        repo_path: Path to the git repository.
        ref: Git commit reference to inspect.

    Returns:
        A set of (crate_name, version) tuples for crates with a source in Cargo.lock.
    """
    cmd = f"git show {ref}:third_party/rust_crates/Cargo.lock"
    content = run(cmd, cwd=repo_path)
    data = tomllib.loads(content)
    crates: Set[Tuple[str, str]] = set()
    for pkg in data.get("package", []):
        if pkg.get("source"):
            crates.add((pkg["name"], pkg["version"]))
    return crates


def group_by_name(crates: Iterable[Tuple[str, str]]) -> Dict[str, List[str]]:
    """Group vendored crate (name, version) tuples by crate package name.

    Args:
        crates: Iterable of (crate_name, version) tuples.

    Returns:
        A dictionary mapping crate package names to lists of version strings.
    """
    grouped: Dict[str, List[str]] = {}
    for name, version in crates:
        grouped.setdefault(name, []).append(version)
    return grouped


def detect_copies_and_moves(
    old_crates: Set[Tuple[str, str]], new_crates: Set[Tuple[str, str]]
) -> Tuple[List[Tuple[str, str, str]], List[Tuple[str, str, str]]]:
    """Classify vendored crate updates into pure copies and pure moves.

    Args:
        old_crates: Set of (crate_name, version) tuples at the base commit.
        new_crates: Set of (crate_name, version) tuples at the target commit.

    Returns:
        A tuple (copies, moves), where each element is a list of
        (crate_name, old_version, new_version) tuples.
    """
    old_grouped = group_by_name(old_crates)
    new_grouped = group_by_name(new_crates)

    all_names = set(old_grouped.keys()) | set(new_grouped.keys())

    copies: List[Tuple[str, str, str]] = []
    moves: List[Tuple[str, str, str]] = []

    for name in sorted(all_names):
        old_vers = old_grouped.get(name, [])
        new_vers = new_grouped.get(name, [])

        if old_vers and new_vers:
            if old_vers != new_vers:
                if len(old_vers) == 1 and len(new_vers) == 1:
                    moves.append((name, old_vers[0], new_vers[0]))
                elif len(old_vers) == 1 and len(new_vers) > 1:
                    for new_v in sorted(new_vers):
                        if new_v not in old_vers:
                            copies.append((name, old_vers[0], new_v))
                else:
                    added_vers = sorted(set(new_vers) - set(old_vers))
                    removed_vers = sorted(set(old_vers) - set(new_vers))
                    if len(added_vers) == 1 and len(removed_vers) == 1:
                        moves.append((name, removed_vers[0], added_vers[0]))
                    elif len(added_vers) == len(removed_vers) and len(added_vers) > 1:
                        for r_v, a_v in zip(removed_vers, added_vers):
                            moves.append((name, r_v, a_v))
                    elif len(added_vers) >= 1 and len(removed_vers) == 0:
                        closest_old = sorted(old_vers)[-1]
                        for added_v in added_vers:
                            copies.append((name, closest_old, added_v))
                    elif len(added_vers) > 1 and len(removed_vers) == 1:
                        moves.append((name, removed_vers[0], added_vers[0]))
                        for added_v in added_vers[1:]:
                            copies.append((name, sorted(old_vers)[-1], added_v))

    return copies, moves


def discover_copies_and_moves(
    repo_path: str, base_ref: str, target_ref: str
) -> Tuple[List[Tuple[str, str, str]], List[Tuple[str, str, str]]]:
    """Compare vendored crates between base and target commits to classify copies and moves.

    Args:
        repo_path: Path to the git repository.
        base_ref: Base git commit SHA or reference.
        target_ref: Target git commit SHA or reference.

    Returns:
        A tuple of (copies, moves), where each element is a list of tuples
        (crate_name, old_version, new_version).
    """
    old_crates = get_crates(repo_path, base_ref)
    new_crates = get_crates(repo_path, target_ref)
    return detect_copies_and_moves(old_crates, new_crates)


def get_all_actions(
    copies: List[Tuple[str, str, str]],
    moves: List[Tuple[str, str, str]],
) -> List[Tuple[str, str, str, str]]:
    """Combine copies and moves into a sorted list of actions.

    Args:
        copies: List of (name, old_version, new_version) copy tuples.
        moves: List of (name, old_version, new_version) move tuples.

    Returns:
        Sorted list of (action_type, name, old_version, new_version) tuples.
    """
    actions: List[Tuple[str, str, str, str]] = []
    for name, old_v, new_v in copies:
        actions.append(("copy", name, old_v, new_v))
    for name, old_v, new_v in moves:
        actions.append(("move", name, old_v, new_v))
    return sorted(actions, key=lambda item: (item[1], item[0], item[2], item[3]))


def discover_operations(
    repo_path: str, base_ref: str, target_ref: str
) -> List[Tuple[str, str, str, str]]:
    """Discover all copy and move operations sorted alphabetically by crate name.

    Args:
        repo_path: Path to the git repository.
        base_ref: Base git commit SHA or reference.
        target_ref: Target git commit SHA or reference.

    Returns:
        A sorted list of tuples (operation_type, crate_name, old_version, new_version).
    """
    copies, moves = discover_copies_and_moves(repo_path, base_ref, target_ref)
    return get_all_actions(copies, moves)


def format_copy_move_commit_message(
    base_ref: str,
    copies: List[Tuple[str, str, str]],
    moves: List[Tuple[str, str, str]],
    vendor_dir: str = "third_party/rust_crates/vendor",
) -> str:
    """Format a detailed commit message listing crate operations and verification commands.

    Args:
        base_ref: Base git reference (e.g. 'origin/main').
        copies: List of (crate_name, old_version, new_version) tuples.
        moves: List of (crate_name, old_version, new_version) tuples.
        vendor_dir: Path to vendored crate directory.

    Returns:
        Formatted commit message string.
    """
    lines = [
        "[rust-3p] Copy and move vendored crates",
        "",
        "This commit contains pure copies and moves of vendored crates under",
        f"{vendor_dir}/ prior to applying dependency updates.",
        "",
    ]

    if copies:
        lines.append("Copies:")
        grouped_copies: Dict[str, List[str]] = {}
        for name, old_v, new_v in copies:
            grouped_copies.setdefault(name, []).append(f"{old_v} -> {new_v}")
        for name in sorted(grouped_copies.keys()):
            vers = ", ".join(grouped_copies[name])
            lines.append(f"* {name}: {vers}")
        lines.append("")

    if moves:
        lines.append("Moves:")
        for name, old_v, new_v in sorted(moves):
            lines.append(f"* {name}: {old_v} -> {new_v}")
        lines.append("")

    lines.append("Verification commands (verify no content changes between base and target names):")
    lines.append("")
    lines.append("Tree-wide check:")
    lines.append(f"  git diff --raw -M100% -C100% --find-copies-harder {base_ref} HEAD")
    lines.append("")
    lines.append("Per-crate directory check (each command should produce empty output):")

    all_actions = get_all_actions(copies, moves)
    for _, name, old_v, new_v in all_actions:
        old_p = crate_vendor_path(name, old_v, vendor_dir=vendor_dir)
        new_p = crate_vendor_path(name, new_v, vendor_dir=vendor_dir)
        lines.append(f"  git diff {base_ref}:{old_p} HEAD:{new_p}")

    return "\n".join(lines)



def crate_vendor_path(
    crate_name: str,
    version: str,
    vendor_dir: str = "third_party/rust_crates/vendor",
) -> str:
    """Return the filesystem path to a vendored crate directory.

    Build metadata '+' characters in package versions are mapped to '-' in
    vendored directory names (e.g. 0.38.0+1.3.281 -> 0.38.0-1.3.281).
    """
    dir_version = version.replace("+", "-")
    return os.path.join(vendor_dir, f"{crate_name}-{dir_version}")


def apply_copy(
    repo_path: str,
    crate_name: str,
    old_v: str,
    new_v: str,
    vendor_dir: str = "third_party/rust_crates/vendor",
    base_ref: Optional[str] = None,
) -> None:
    """Apply a crate directory copy in the working tree and stage it.

    Args:
        repo_path: Path to the git repository.
        crate_name: Name of the crate package.
        old_v: Existing version string to copy from.
        new_v: Target version string to create.
        vendor_dir: Relative path to the vendor directory.
        base_ref: Optional git reference to restore source directory from if missing.
    """
    old_path = crate_vendor_path(crate_name, old_v, vendor_dir=vendor_dir)
    new_path = crate_vendor_path(crate_name, new_v, vendor_dir=vendor_dir)
    if base_ref and not os.path.exists(os.path.join(repo_path, old_path)):
        run(f"git checkout {base_ref} -- {old_path}", cwd=repo_path)
    run(f"rm -rf {new_path}", cwd=repo_path)
    run(f"cp -r {old_path} {new_path}", cwd=repo_path)
    run(f"git add {new_path}", cwd=repo_path)


def apply_move(
    repo_path: str,
    base_ref: str,
    crate_name: str,
    old_v: str,
    new_v: str,
    vendor_dir: str = "third_party/rust_crates/vendor",
) -> None:
    """Apply a crate directory move in the working tree and stage it.

    Args:
        repo_path: Path to the git repository.
        base_ref: Base git reference to restore source crate directory from if needed.
        crate_name: Name of the crate package.
        old_v: Source version string to move from.
        new_v: Destination version string to move to.
        vendor_dir: Relative path to the vendor directory.
    """
    old_path = crate_vendor_path(crate_name, old_v, vendor_dir=vendor_dir)
    new_path = crate_vendor_path(crate_name, new_v, vendor_dir=vendor_dir)
    run(f"git checkout {base_ref} -- {old_path}", cwd=repo_path)
    run(f"rm -rf {new_path}", cwd=repo_path)
    run(f"git add {old_path}", cwd=repo_path)
    run(f"git mv {old_path} {new_path}", cwd=repo_path)


def apply_copies(
    repo_path: str,
    copies: List[Tuple[str, str, str]],
    vendor_dir: str = "third_party/rust_crates/vendor",
    base_ref: Optional[str] = None,
) -> None:
    """Apply pure copy operations for vendored crates in the repository.

    Args:
        repo_path: Absolute path to the Git repository.
        copies: List of (crate_name, old_version, new_version) tuples.
        vendor_dir: Relative path to the vendor directory.
        base_ref: Optional git reference to restore source directories from if missing.
    """
    for name, old_v, new_v in copies:
        apply_copy(repo_path, name, old_v, new_v, vendor_dir=vendor_dir, base_ref=base_ref)


def apply_moves(
    repo_path: str,
    base_hash: str,
    moves: List[Tuple[str, str, str]],
    vendor_dir: str = "third_party/rust_crates/vendor",
) -> None:
    """Apply pure move operations for vendored crates in the repository.

    Args:
        repo_path: Absolute path to the Git repository.
        base_hash: Commit hash of the base commit to check out old paths from.
        moves: List of (crate_name, old_version, new_version) tuples.
        vendor_dir: Relative path to the vendor directory.
    """
    for name, old_v, new_v in moves:
        apply_move(
            repo_path, base_hash, name, old_v, new_v, vendor_dir=vendor_dir
        )


def apply_actions(
    repo_path: str,
    base_hash: str,
    actions: List[Tuple[str, str, str, str]],
    vendor_dir: str = "third_party/rust_crates/vendor",
) -> None:
    """Apply a sequence of copy and move actions for vendored crates.

    Args:
        repo_path: Absolute path to the Git repository.
        base_hash: Commit hash of the base commit.
        actions: List of (action_type, crate_name, old_version, new_version) tuples.
        vendor_dir: Relative path to the vendor directory.
    """
    for action_type, name, old_v, new_v in actions:
        if action_type == "copy":
            apply_copy(
                repo_path, name, old_v, new_v, vendor_dir=vendor_dir, base_ref=base_hash
            )
        elif action_type == "move":
            apply_move(
                repo_path, base_hash, name, old_v, new_v, vendor_dir=vendor_dir
            )
        else:
            raise ValueError(f"Unknown action type: {action_type}")


def finalize_actual_updates(
    repo_path: str,
    base_hash: str,
    target_hash: str,
    use_revert: bool = False,
    vendor_dir: str = "third_party/rust_crates/vendor",
) -> str:
    """Finalize the commit containing actual updates on top of copy/move commit(s).

    Creates a temporary commit restoring the exact tree state of base_hash so that
    cherry-picking target_hash applies cleanly without tree conflicts. Then resets
    softly back to the last copy/move commit and commits with target_hash's metadata.

    Args:
        repo_path: Absolute path to the Git repository.
        base_hash: Commit hash of the base commit.
        target_hash: Commit hash of the target commit to cherry-pick.
        use_revert: Retained for API compatibility; tree restoration is performed reliably.
        vendor_dir: Relative path to the vendor directory.

    Returns:
        The commit hash of the resulting actual updates commit.
    """
    last_copy_commit = resolve_commit(repo_path, "HEAD")
    base_tree = run(f"git rev-parse {base_hash}^{{tree}}", cwd=repo_path)
    restore_commit = run(
        f'git commit-tree {base_tree} -p HEAD -m "[rust-3p] Temporary restore base tree"',
        cwd=repo_path,
    )
    run(f"git reset --hard {restore_commit}", cwd=repo_path)

    try:
        run(f"git cherry-pick {base_hash}..{target_hash}", cwd=repo_path)
    except subprocess.CalledProcessError:
        try:
            run("git cherry-pick --abort", cwd=repo_path)
        except Exception:
            pass
        run(f"git cherry-pick {target_hash}", cwd=repo_path)

    run(f"git reset --soft {last_copy_commit}", cwd=repo_path)
    run(f"git commit -C {target_hash}", cwd=repo_path)
    return resolve_commit(repo_path, "HEAD")


def group_operations(
    operations: List[Tuple[str, str, str, str]],
    batch_size: Optional[int] = None,
    num_groups: Optional[int] = None,
    groups_file: Optional[str] = None,
) -> List[List[Tuple[str, str, str, str]]]:
    """Group copy and move operations into ordered sequential commit batches.

    Args:
        operations: List of (op_type, crate_name, old_version, new_version) tuples.
        batch_size: Maximum number of crate operations per group.
        num_groups: Number of sequential groups to partition operations into.
        groups_file: Path to JSON configuration file defining groups of crate names.

    Returns:
        A list of groups, where each group is a list of operation tuples.
    """
    if not operations:
        return []

    if groups_file:
        with open(groups_file, "r", encoding="utf-8") as f:
            raw_groups = json.load(f)

        group_entries: List[Any] = []
        if isinstance(raw_groups, dict):
            group_entries = list(raw_groups.values())
        elif isinstance(raw_groups, list):
            group_entries = raw_groups

        assigned: Set[int] = set()
        grouped_result: List[List[Tuple[str, str, str, str]]] = []

        for group_entry in group_entries:
            group_names = set()
            if isinstance(group_entry, list):
                for item in group_entry:
                    if isinstance(item, dict) and "name" in item:
                        group_names.add(item["name"])
                    elif isinstance(item, str):
                        group_names.add(item)
            elif isinstance(group_entry, dict):
                crates_list = (
                    group_entry.get("crates")
                    or group_entry.get("names")
                    or []
                )
                for item in crates_list:
                    if isinstance(item, str):
                        group_names.add(item)

            current_group: List[Tuple[str, str, str, str]] = []
            for i, op in enumerate(operations):
                if i not in assigned and op[1] in group_names:
                    current_group.append(op)
                    assigned.add(i)

            if current_group:
                grouped_result.append(current_group)

        unassigned = [
            op for i, op in enumerate(operations) if i not in assigned
        ]
        if unassigned:
            grouped_result.append(unassigned)

        return grouped_result

    if num_groups is not None and num_groups > 0:
        k = max(1, min(num_groups, len(operations)))
        grouped_result = []
        n = len(operations)
        for i in range(k):
            start = (i * n) // k
            end = ((i + 1) * n) // k
            if start < end:
                grouped_result.append(operations[start:end])
        return grouped_result

    size = batch_size if (batch_size is not None and batch_size > 0) else 10
    return [operations[i : i + size] for i in range(0, len(operations), size)]
