# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia platform hook action adapters.

Integrates Fuchsia platform formatting and commit message checking
with the generic git_staging engine.
"""

from __future__ import annotations

import shutil
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from pathlib import Path

from agents.lib import git_staging
from agents.lib.git_staging.engine import run_cmd
from agents.lib.githooks.reporters import ConsoleReporter
from agents.lib.paths import find_fuchsia_dir

DEFAULT_FORMATTABLE_EXTENSIONS: tuple[str, ...] = (
    ".py",
    ".md",
    ".c",
    ".cc",
    ".cpp",
    ".h",
    ".hh",
    ".hpp",
    ".rs",
    ".go",
    ".fidl",
    ".gn",
    ".gni",
    ".json",
    ".json5",
    ".cml",
    ".proto",
    ".ts",
)


@dataclass(frozen=True)
class HookContext(git_staging.ActionContext):
    """Action context for Fuchsia git hooks including agent execution flag."""

    is_agent: bool = False
    reporter: ConsoleReporter = field(default_factory=ConsoleReporter)


@dataclass(frozen=True)
class HookAction:
    """Action executed on staged or checked repository files.

    Attributes:
        name: Identifier for the action.
        action_fn: Callable[[HookContext, Sequence[str]], bool]
        is_mutating: Whether the action modifies files on disk.
        extensions: File extensions to filter for (None matches all files).
        remediation_cmd_fn: Optional callable to generate remediation commands.
    """

    name: str
    action_fn: Callable[[HookContext, Sequence[str]], bool]
    is_mutating: bool
    extensions: Sequence[str] | None = None
    remediation_cmd_fn: Callable[[Sequence[str]], Sequence[str]] | None = None


def format_code_action(
    context: HookContext,
    files: Sequence[str],
) -> bool:
    """Executes fx format-code on target files.

    Args:
        context: HookContext with repo_root, check_only, is_agent, and reporter.
        files: Sequence of file paths relative to repo_root.

    Returns:
        True if formatting succeeded or fail-opened, False otherwise.
    """
    if not files:
        return True

    reporter = context.reporter
    is_agent = context.is_agent

    fx_cmd = None
    try:
        candidate = find_fuchsia_dir(context.repo_root) / "scripts" / "fx"
        if candidate.is_file():
            fx_cmd = str(candidate)
    except RuntimeError:
        pass

    if fx_cmd is None and shutil.which("fx"):
        fx_cmd = "fx"

    if fx_cmd is None:
        if not is_agent:
            reporter.on_warning(
                "`fx` not found on PATH or in scripts/fx. Skipping code formatting."
            )
            return True
        reporter.on_error(
            "`fx` command not found on PATH or in scripts/fx in agent environment."
        )
        return False

    cmd = [fx_cmd, "format-code", f"--files={','.join(files)}"]

    res = run_cmd(cmd, cwd=context.repo_root)

    if res.returncode != 0:
        output = "\n".join(
            filter(None, [res.stdout.strip(), res.stderr.strip()])
        )
        if not output:
            output = f"Command {cmd[0]} failed with exit code {res.returncode}"
        reporter.on_error(output)
        return False

    if context.check_only:
        diff_res = run_cmd(
            ["git", "diff", "--quiet", "--", *files],
            cwd=context.repo_root,
        )
        if diff_res.returncode != 0:
            return False

    return True


DEFAULT_PRE_COMMIT_ACTIONS: tuple[HookAction, ...] = (
    HookAction(
        name="fx_format_code",
        action_fn=format_code_action,
        is_mutating=True,
        extensions=DEFAULT_FORMATTABLE_EXTENSIONS,
        remediation_cmd_fn=lambda files: [
            f"fx format-code --files={','.join(files)}"
        ],
    ),
)


def commit_msg_action(
    context: HookContext,
    files: Sequence[str],
) -> bool:
    """Executes commit_msg_checker.py on a commit message file.

    Args:
        context: HookContext with repo_root, is_agent, and reporter.
        files: Sequence of file paths containing the commit message file.

    Returns:
        True if commit message check passed, False otherwise.
    """
    if not files:
        return True

    reporter = context.reporter
    is_agent = context.is_agent

    msg_file = Path(files[0])
    try:
        fuchsia_dir = find_fuchsia_dir(context.repo_root)
    except RuntimeError:
        fuchsia_dir = context.repo_root

    checker_script = fuchsia_dir / "scripts" / "shac" / "commit_msg_checker.py"
    if not checker_script.is_file():
        if is_agent:
            reporter.on_error(
                f"Commit message checker script not found: {checker_script}"
            )
            return False
        reporter.on_warning(
            f"Commit message checker script not found: {checker_script}. Skipping check."
        )
        return True

    python_bin = fuchsia_dir / "scripts" / "fuchsia-vendored-python"
    py_exec = str(python_bin) if python_bin.is_file() else sys.executable

    cmd = [py_exec, str(checker_script), "--message-file", str(msg_file)]
    if is_agent:
        cmd.append("--strict")

    res = run_cmd(cmd, cwd=context.repo_root)

    if res.returncode != 0:
        output = "\n".join(
            filter(None, [res.stdout.strip(), res.stderr.strip()])
        )
        if not output:
            output = f"Command {cmd[0]} failed with exit code {res.returncode}"
        reporter.on_error(output)
        return False

    # In non-strict mode (human developer), surface advisory warnings if any
    output = res.stdout.strip()
    if output and not is_agent:
        reporter.on_warning(output)

    return True
