# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Installer and uninstaller for Fuchsia git hooks."""

from __future__ import annotations

import shlex
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path

from agents.lib.paths import find_checkout_git_repos, get_hooks_dir

__all__ = [
    "HookOperationResult",
    "HookStatusResult",
    "get_git_hooks_status",
    "install_git_hook",
    "install_git_hooks",
    "uninstall_git_hook",
    "uninstall_git_hooks",
]


@dataclass(frozen=True)
class HookOperationResult:
    """Result of installing or uninstalling Fuchsia git hooks across repositories.

    Attributes:
        modified: List of modified (installed or uninstalled) hook file paths.
        failed: List of (repository_root, error_message) for repositories that failed.
    """

    modified: list[Path]
    failed: list[tuple[Path, str]] = field(default_factory=list)


@dataclass(frozen=True)
class HookStatusResult:
    """Result of querying Git hooks configuration status across repositories.

    Attributes:
        total_repos: Total number of active checkout Git repositories discovered.
        configured_repos: Number of repositories with Fuchsia git hooks configured.
    """

    total_repos: int
    configured_repos: int


AGENT_FRAGMENT_NAME = "10-fuchsia-agent.sh"

AGENT_FRAGMENT_TEMPLATE = """#!/bin/sh
# ==============================================================================
# @managed-by: fx agents
# ==============================================================================

[ "$FUCHSIA_SKIP_HOOKS" = "1" ] && exit 0

find_fuchsia_root() {{
  DIR="$1"
  while [ "$DIR" != "/" ] && [ "$DIR" != "." ]; do
    if [ -d "$DIR/.jiri_root" ] || [ -f "$DIR/.fx-root" ]; then
      FUCHSIA_DIR="$DIR"
      return 0
    fi
    DIR="$(dirname "$DIR")"
  done
  return 1
}}

if [ -z "$FUCHSIA_DIR" ]; then
  find_fuchsia_root "$PWD"
  if [ -z "$FUCHSIA_DIR" ]; then
    COMMON_DIR="$(git rev-parse --git-common-dir 2>/dev/null)"
    if [ -n "$COMMON_DIR" ]; then
      find_fuchsia_root "$(cd "$COMMON_DIR/.." 2>/dev/null && pwd)"
    fi
  fi
fi

if [ -n "$FUCHSIA_DIR" ] && [ -x "$FUCHSIA_DIR/scripts/fuchsia-vendored-python" ] && [ -f "{script_path}" ]; then
  export FUCHSIA_DIR
  export PATH="$FUCHSIA_DIR/.jiri_root/bin:$FUCHSIA_DIR/scripts:$PATH"
  export PYTHONPATH="$FUCHSIA_DIR/tools${{PYTHONPATH:+:$PYTHONPATH}}"
  exec "$FUCHSIA_DIR/scripts/fuchsia-vendored-python" "{script_path}"{args_str} "$@"
fi
exit 0
"""


def install_git_hook(
    repo_dir: Path,
    hook_name: str,
    script_path: Path | str,
    script_args: Sequence[str] = (),
    dry_run: bool = False,
    hooks_dir: Path | None = None,
) -> Path:
    """Installs a git hook fragment in a repository's hooks.d directory.

    Args:
        repo_dir: Path to the root of the repository.
        hook_name: Name of the git hook (e.g. 'pre-commit').
        script_path: Path to the script that the hook should run.
        script_args: Arguments to prepend when calling the script.
        dry_run: If True, do not modify files on disk.
        hooks_dir: Pre-resolved git hooks directory path (resolves if None).

    Returns:
        Path to the installed hook fragment.
    """
    if hooks_dir is None:
        hooks_dir = get_hooks_dir(repo_dir)
    hook_d = hooks_dir / f"{hook_name}.d"
    agent_fragment = hook_d / AGENT_FRAGMENT_NAME

    if dry_run:
        return agent_fragment

    hook_d.mkdir(parents=True, exist_ok=True)

    args_str = (
        f" {' '.join(shlex.quote(a) for a in script_args)}"
        if script_args
        else ""
    )
    final_fragment_content = AGENT_FRAGMENT_TEMPLATE.format(
        script_path=script_path, args_str=args_str
    )

    agent_fragment.write_text(final_fragment_content, encoding="utf-8")
    agent_fragment.chmod(0o755)

    return agent_fragment


def uninstall_git_hook(
    repo_dir: Path,
    hook_name: str,
    dry_run: bool = False,
    hooks_dir: Path | None = None,
) -> Path | None:
    """Uninstalls a git hook fragment from a repository's hooks.d directory.

    Args:
        repo_dir: Path to the root of the repository.
        hook_name: Name of the git hook (e.g. 'pre-commit').
        dry_run: If True, do not modify files on disk.
        hooks_dir: Pre-resolved git hooks directory path (resolves if None).

    Returns:
        Path to the uninstalled hook fragment, or None if it wasn't installed.
    """
    if hooks_dir is None:
        hooks_dir = get_hooks_dir(repo_dir)
    agent_fragment = hooks_dir / f"{hook_name}.d" / AGENT_FRAGMENT_NAME

    if not agent_fragment.is_file():
        return None

    if not dry_run:
        agent_fragment.unlink()
    return agent_fragment


def install_git_hooks(
    fuchsia_dir: Path,
    hook_names: Sequence[str] = ("pre-commit", "commit-msg"),
    dry_run: bool = False,
) -> HookOperationResult:
    """Installs Fuchsia git hooks across all discovered Git repositories in checkout.

    Args:
        fuchsia_dir: Root directory of the Fuchsia checkout.
        hook_names: Base hook names to install (e.g. 'pre-commit', 'commit-msg').
        dry_run: If True, do not modify files on disk.

    Returns:
        HookOperationResult containing installed hook file paths and any failures.
    """
    repos = find_checkout_git_repos(fuchsia_dir)
    formatted_script_path = "$FUCHSIA_DIR/tools/agents/lib/githooks/runner.py"

    modified: list[Path] = []
    failed: list[tuple[Path, str]] = []
    for repo in repos:
        try:
            hooks_dir = get_hooks_dir(repo)
        except (RuntimeError, OSError) as e:
            failed.append((repo, str(e)))
            continue
        for hook_name in hook_names:
            script_args = [hook_name]
            try:
                hook_path = install_git_hook(
                    repo,
                    hook_name=hook_name,
                    script_path=formatted_script_path,
                    script_args=script_args,
                    dry_run=dry_run,
                    hooks_dir=hooks_dir,
                )
                modified.append(hook_path)
            except (RuntimeError, OSError) as e:
                failed.append((repo, str(e)))
    return HookOperationResult(modified=modified, failed=failed)


def uninstall_git_hooks(
    fuchsia_dir: Path,
    hook_names: Sequence[str] = ("pre-commit", "commit-msg"),
    dry_run: bool = False,
) -> HookOperationResult:
    """Uninstalls Fuchsia git hooks across all discovered Git repositories in checkout.

    Args:
        fuchsia_dir: Root directory of the Fuchsia checkout.
        hook_names: Base hook names to uninstall (e.g. 'pre-commit', 'commit-msg').
        dry_run: If True, do not modify files on disk.

    Returns:
        HookOperationResult containing uninstalled hook file paths and any failures.
    """
    repos = find_checkout_git_repos(fuchsia_dir)
    modified: list[Path] = []
    failed: list[tuple[Path, str]] = []
    for repo in repos:
        try:
            hooks_dir = get_hooks_dir(repo)
        except (RuntimeError, OSError) as e:
            failed.append((repo, str(e)))
            continue
        for hook_name in hook_names:
            try:
                removed = uninstall_git_hook(
                    repo,
                    hook_name=hook_name,
                    dry_run=dry_run,
                    hooks_dir=hooks_dir,
                )
                if removed is not None:
                    modified.append(removed)
            except (RuntimeError, OSError) as e:
                failed.append((repo, str(e)))
    return HookOperationResult(modified=modified, failed=failed)


def get_git_hooks_status(
    fuchsia_dir: Path,
    hook_names: Sequence[str] = ("pre-commit", "commit-msg"),
) -> HookStatusResult:
    """Checks Git hooks configuration status across checkout repositories.

    A repository is considered configured if the Fuchsia agent hook fragment
    exists in the hook's .d directory for all specified hook names.

    Args:
        fuchsia_dir: Root directory of the Fuchsia checkout.
        hook_names: Base hook names to check (defaults to 'pre-commit' and 'commit-msg').

    Returns:
        HookStatusResult with total and configured repository counts.
    """
    repos = find_checkout_git_repos(fuchsia_dir)
    configured = 0
    for repo in repos:
        try:
            h_dir = get_hooks_dir(repo)
            if hook_names and all(
                (h_dir / f"{h}.d" / AGENT_FRAGMENT_NAME).is_file()
                for h in hook_names
            ):
                configured += 1
        except (RuntimeError, OSError):
            pass
    return HookStatusResult(total_repos=len(repos), configured_repos=configured)
