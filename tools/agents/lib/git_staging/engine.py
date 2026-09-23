# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generic Git staging isolation and staged action runner library.

This module provides core primitives for safely working with Git index and staged files:
1. Staging partition: identifies fully staged vs. partially staged files.
2. Temporary stash isolation: uses `$S^2 -> S diff apply` to isolate staged edits
   without triggering false 3-way merge conflicts against HEAD or corrupting unstaged hunks.
3. Staged action pipeline: orchestrates mutating actions on fully staged files (with automatic
   re-staging) and read-only checks on partially staged files under stash isolation.
"""

from __future__ import annotations

import dataclasses
import subprocess
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any


class GitError(RuntimeError):
    """Exception raised for Git staging and hook operations."""


@dataclasses.dataclass(frozen=True, slots=True)
class ActionContext:
    """Context passed to hook actions and staged pipelines."""

    repo_root: Path
    check_only: bool = False


@dataclasses.dataclass(frozen=True, slots=True)
class PipelineResult:
    """Result of executing a git hook action or pipeline."""

    success: bool
    restaged_files: tuple[str, ...] = ()
    partial_conflicts: tuple[str, ...] = ()
    error: str = ""

    @classmethod
    def ok(cls, restaged_files: Sequence[str] = ()) -> PipelineResult:
        """Constructs a successful PipelineResult."""
        return cls(success=True, restaged_files=tuple(restaged_files))

    @classmethod
    def fail(
        cls,
        error: str = "",
        partial_conflicts: Sequence[str] = (),
        restaged_files: Sequence[str] = (),
    ) -> PipelineResult:
        """Constructs a failed PipelineResult."""
        return cls(
            success=False,
            restaged_files=tuple(restaged_files),
            partial_conflicts=tuple(partial_conflicts),
            error=error,
        )


# Action callable signature: (context: ActionContext, files: Sequence[str]) -> bool
StagedFileAction = Callable[[ActionContext, Sequence[str]], bool]


def run_cmd(
    cmd: Sequence[str],
    *,
    cwd: Path | None = None,
    input_data: str | bytes | None = None,
    text: bool = True,
) -> subprocess.CompletedProcess[Any]:
    """Runs a subprocess command with captured output."""
    return subprocess.run(
        list(cmd),
        cwd=cwd,
        input=input_data,
        capture_output=True,
        text=text,
    )


def get_stash_sha(repo_dir: Path) -> str | None:
    """Returns the commit hash of the current stash, or None if no stash exists.

    Args:
        repo_dir: Root directory of the repository.

    Returns:
        The commit hash string of refs/stash, or None if refs/stash does not exist.
    """
    res = run_cmd(
        ["git", "rev-parse", "-q", "--verify", "refs/stash"], cwd=repo_dir
    )
    return (
        res.stdout.strip()
        if res.returncode == 0 and res.stdout.strip()
        else None
    )


def get_modified_files(repo_root: Path, *, staged: bool = False) -> set[str]:
    """Returns set of modified file paths relative to repo root.

    Args:
        repo_root: Root directory of the repository.
        staged: If True, returns staged files; otherwise, returns unstaged files.

    Returns:
        Set of modified file paths relative to repo root.
    """
    cmd = ["git", "diff"]
    if staged:
        cmd.extend(["--cached", "--diff-filter=ACMR"])
    cmd.extend(["--name-only", "-z"])

    res = run_cmd(cmd, cwd=repo_root)
    if res.returncode != 0:
        err = res.stderr.strip() or "git diff failed"
        raise GitError(f"Failed to query modified files: {err}")
    return {line for line in res.stdout.split("\0") if line}


@dataclasses.dataclass(frozen=True, slots=True)
class StagedPartition:
    """Partition of staged files into fully and partially staged sets."""

    fully_staged: frozenset[str]
    partially_staged: frozenset[str]


def partition_staged_files(
    repo_root: Path,
    extensions: Sequence[str] | None = None,
) -> StagedPartition:
    """Partitions staged files into fully and partially staged sets.

    Args:
        repo_root: Root directory of the repository.
        extensions: Optional file extensions filter (e.g. ('.py', '.cc')).

    Returns:
        StagedPartition containing fully_staged and partially_staged sets.
    """
    exts = tuple(extensions) if extensions else None
    staged = get_modified_files(repo_root, staged=True)
    unstaged = get_modified_files(repo_root, staged=False)

    if exts:
        staged = {f for f in staged if f.endswith(exts)}

    return StagedPartition(
        fully_staged=frozenset(staged - unstaged),
        partially_staged=frozenset(staged & unstaged),
    )


_DEFAULT_STASH_MESSAGE = "git-staging-isolation"


class StashGuard:
    """Context manager for temporary Git stash isolation using $S^2 -> S diff apply.

    Temporarily stashes unstaged modifications to isolate staged changes without
    touching untracked files or triggering 3-way merge conflicts.
    """

    def __init__(self, repo_dir: Path) -> None:
        self.repo_dir = repo_dir
        self.stashed = False
        self.stash_sha: str | None = None
        self._unstaged_deleted_files: tuple[str, ...] = ()

    def __enter__(self) -> StashGuard:
        head_res = run_cmd(
            ["git", "rev-parse", "--verify", "-q", "HEAD"], cwd=self.repo_dir
        )
        if head_res.returncode != 0:
            raise GitError(
                f"Cannot isolate partial staging on an unborn branch in {self.repo_dir}. "
                "Stage all files or commit all changes for the initial commit."
            )

        del_res = run_cmd(
            ["git", "diff", "--name-only", "--diff-filter=D", "-z"],
            cwd=self.repo_dir,
        )
        if del_res.returncode != 0:
            err = del_res.stderr.strip() or "git diff failed"
            raise GitError(
                f"Failed to query unstaged deletions in {self.repo_dir}: {err}"
            )
        self._unstaged_deleted_files = tuple(
            f for f in del_res.stdout.split("\0") if f
        )

        stash_before = get_stash_sha(self.repo_dir)
        stash_res = run_cmd(
            [
                "git",
                "stash",
                "push",
                "--keep-index",
                "-q",
                "-m",
                _DEFAULT_STASH_MESSAGE,
            ],
            cwd=self.repo_dir,
        )
        self.stash_sha = get_stash_sha(self.repo_dir)
        if stash_res.returncode == 0:
            if self.stash_sha is not None and self.stash_sha != stash_before:
                self.stashed = True
            else:
                self.stashed = False
                self.stash_sha = None
        else:
            err = stash_res.stderr.strip() or "git stash push failed"
            raise GitError(
                f"Failed to stash unstaged changes for isolation in {self.repo_dir}: {err}"
            )
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: Any,
    ) -> None:
        if not self.stashed or not self.stash_sha:
            return

        try:
            stash_sha = self.stash_sha
            stash_diff = run_cmd(
                [
                    "git",
                    "diff",
                    "--no-color",
                    "--binary",
                    f"{stash_sha}^2",
                    stash_sha,
                ],
                cwd=self.repo_dir,
                text=False,
            )
            if stash_diff.returncode != 0:
                err = stash_diff.stderr.decode(
                    "utf-8", errors="replace"
                ).strip()
                err_msg = f"Failed to diff temporary stash {stash_sha}: {err}"
                raise GitError(err_msg) from exc_val

            # Ensure tracked working tree matches index (S^2) before applying diff,
            # even if a check tool inadvertently mutated files on disk.
            checkout_res = run_cmd(
                ["git", "checkout", "--force", "--", "."], cwd=self.repo_dir
            )
            if checkout_res.returncode != 0:
                err = checkout_res.stderr.strip() or "git checkout failed"
                raise GitError(
                    f"Failed to reset working tree to index in {self.repo_dir}: {err}"
                ) from exc_val

            apply_res = run_cmd(
                ["git", "apply", "--binary", "--allow-empty"],
                cwd=self.repo_dir,
                input_data=stash_diff.stdout,
                text=False,
            )
            if apply_res.returncode == 0:
                if get_stash_sha(self.repo_dir) == stash_sha:
                    run_cmd(["git", "stash", "drop", "-q"], cwd=self.repo_dir)
                self.stashed = False
                self.stash_sha = None
                return

            err = apply_res.stderr.decode("utf-8", errors="replace").strip()
            err_msg = (
                f"Could not automatically restore temporary stash {stash_sha} "
                f"(preserved at stash@{{0}}): {err}\n"
                f"To restore your unstaged changes, run: git stash apply stash@{{0}}"
            )
            raise GitError(err_msg) from exc_val
        # Git stash push --keep-index omits unstaged deletions of newly added index entries
        # from the stash diff. Manually ensure these files remain deleted in the working tree.
        finally:
            for rel_path in self._unstaged_deleted_files:
                target = self.repo_dir / rel_path
                target.unlink(missing_ok=True)


def run_staged_pipeline(
    context: ActionContext,
    mutating_actions: Sequence[StagedFileAction] = (),
    read_only_checks: Sequence[StagedFileAction] = (),
    extensions: Sequence[str] | None = None,
) -> PipelineResult:
    """Executes staged action pipeline with safe stash isolation.

    Args:
        context: ActionContext containing repository root and check_only flags.
        mutating_actions: Actions run on fully staged files (e.g. formatters).
        read_only_checks: Check actions run on fully and partially staged files.
        extensions: Optional file extensions filter for formattable files.

    Returns:
        PipelineResult recording success, restaged files, partial conflicts, or errors.
    """
    partition = partition_staged_files(context.repo_root, extensions)
    if not (partition.fully_staged or partition.partially_staged):
        return PipelineResult.ok()

    restaged_files: list[str] = []
    check_ctx = dataclasses.replace(context, check_only=True)

    # 1. Run mutating actions and read-only checks on fully staged files
    if partition.fully_staged:
        sorted_fully = sorted(partition.fully_staged)
        for action in mutating_actions:
            if not action(context, sorted_fully):
                # Revert any unstaged modifications made by failed or prior actions
                # to preserve transaction safety and prevent leaving dirty unstaged files.
                run_cmd(
                    [
                        "git",
                        "checkout",
                        "--force",
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ],
                    cwd=context.repo_root,
                    input_data="\0".join(sorted_fully) + "\0",
                )
                return PipelineResult.fail(error="Mutating action failed")

        if mutating_actions and not context.check_only:
            try:
                actually_modified = sorted(
                    get_modified_files(context.repo_root, staged=False)
                    & partition.fully_staged
                )
            except GitError as e:
                return PipelineResult.fail(error=str(e))
            if actually_modified:
                res_add = run_cmd(
                    [
                        "git",
                        "add",
                        "--pathspec-from-file=-",
                        "--pathspec-file-nul",
                    ],
                    cwd=context.repo_root,
                    input_data="\0".join(actually_modified) + "\0",
                )
                if res_add.returncode != 0:
                    err = (
                        res_add.stderr.strip()
                        or res_add.stdout.strip()
                        or "git add failed"
                    )
                    return PipelineResult.fail(error=err)
                restaged_files.extend(actually_modified)

        for check_action in read_only_checks:
            if not check_action(check_ctx, sorted_fully):
                return PipelineResult.fail(
                    error="Read-only check failed on fully staged files",
                    restaged_files=restaged_files,
                )

    # 2. Run read-only checks under temporary stash isolation
    if partition.partially_staged:
        try:
            with StashGuard(context.repo_root):
                sorted_partial = sorted(partition.partially_staged)
                for action in (*mutating_actions, *read_only_checks):
                    if not action(check_ctx, sorted_partial):
                        return PipelineResult.fail(
                            error="Formatting or check failure on partially staged files",
                            partial_conflicts=sorted_partial,
                            restaged_files=restaged_files,
                        )
        except GitError as e:
            return PipelineResult.fail(
                error=str(e), restaged_files=restaged_files
            )

    return PipelineResult.ok(restaged_files=restaged_files)
