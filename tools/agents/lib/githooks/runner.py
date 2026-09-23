#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia Git hook CLI runner and hook dispatcher entrypoints."""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import cast

# Bootstrap sys.path when executed directly from the source tree under hermetic-env.
_FUCHSIA_DIR = os.environ.get("FUCHSIA_DIR")
_root = (
    Path(_FUCHSIA_DIR) if _FUCHSIA_DIR else Path(__file__).resolve().parents[4]
)
_tools_dir = str(_root / "tools")
if (_root / "tools").is_dir() and _tools_dir not in sys.path:
    sys.path.insert(0, _tools_dir)

from agents.lib import git_staging, paths
from agents.lib.githooks.adapters import (
    DEFAULT_FORMATTABLE_EXTENSIONS,
    DEFAULT_PRE_COMMIT_ACTIONS,
    HookAction,
    HookContext,
    commit_msg_action,
)
from agents.lib.githooks.reporters import ConsoleReporter

AGENT_ENV_VARS: tuple[str, ...] = (
    "ANTIGRAVITY_AGENT",
    "GEMINI_CLI",
    "ANTIGRAVITY_EDITOR_APP_ROOT",
)

_FALSY_VALUES: frozenset[str] = frozenset({"0", "false", "no", "n", "off", ""})


def _is_env_truthy(
    name: str,
    env: Mapping[str, str] | None = None,
) -> bool:
    """Returns True if the environment variable represents a truthy boolean or non-empty string."""
    target_env = env if env is not None else os.environ
    val = target_env.get(name)
    if val is None:
        return False
    cleaned = val.strip().lower()
    if cleaned in _FALSY_VALUES:
        return False
    return True


def is_invoked_by_agent(
    env: Mapping[str, str] | None = None,
) -> bool:
    """Detects whether the process was invoked by an automated AI agent."""
    target_env = env if env is not None else os.environ
    return any(_is_env_truthy(var, env=target_env) for var in AGENT_ENV_VARS)


def run_pre_commit_hook(
    *,
    repo_dir: Path | None = None,
    actions: Sequence[HookAction] = DEFAULT_PRE_COMMIT_ACTIONS,
    reporter: ConsoleReporter | None = None,
    is_agent: bool | None = None,
) -> int:
    """Executes pre-commit hook checks using run_staged_pipeline."""
    if _is_env_truthy("FUCHSIA_SKIP_HOOKS"):
        return 0

    is_agent = is_invoked_by_agent() if is_agent is None else is_agent
    active_reporter = reporter or ConsoleReporter()

    target_dir = repo_dir if repo_dir is not None else Path.cwd()
    try:
        repo_root = paths.get_repo_root(target_dir)
    except RuntimeError as e:
        active_reporter.on_error(str(e))
        active_reporter.finish()
        return 1

    context = HookContext(
        repo_root=repo_root,
        check_only=False,
        is_agent=is_agent,
        reporter=active_reporter,
    )

    if any(a.extensions is None for a in actions):
        exts = None
    else:
        exts = (
            tuple(
                {ext for a in actions if a.extensions for ext in a.extensions}
            )
            or DEFAULT_FORMATTABLE_EXTENSIONS
        )

    def _wrap_action(action: HookAction) -> git_staging.StagedFileAction:
        def wrapped(
            ctx: git_staging.ActionContext, files: Sequence[str]
        ) -> bool:
            matched_files = (
                [f for f in files if f.endswith(tuple(action.extensions))]
                if action.extensions
                else list(files)
            )
            if not matched_files:
                return True
            ok = action.action_fn(cast(HookContext, ctx), matched_files)
            if (
                not ok
                and not (action.is_mutating and ctx.check_only)
                and not active_reporter.has_errors
            ):
                active_reporter.on_error(f"Action '{action.name}' failed.")
            return ok

        return wrapped

    mutating = [_wrap_action(a) for a in actions if a.is_mutating]
    read_only = [_wrap_action(a) for a in actions if not a.is_mutating]

    result = git_staging.run_staged_pipeline(
        context=context,
        mutating_actions=mutating,
        read_only_checks=read_only,
        extensions=exts,
    )

    if result.restaged_files:
        active_reporter.on_files_restaged(
            list(result.restaged_files),
            message="Auto-formatted and re-staged",
        )

    if result.partial_conflicts:
        remediation_cmds: list[str] = []
        for action in actions:
            if action.remediation_cmd_fn is not None:
                matched_conflicts = (
                    [
                        f
                        for f in result.partial_conflicts
                        if f.endswith(tuple(action.extensions))
                    ]
                    if action.extensions
                    else list(result.partial_conflicts)
                )
                if matched_conflicts:
                    remediation_cmds.extend(
                        action.remediation_cmd_fn(matched_conflicts)
                    )
        active_reporter.on_partial_staging_conflict(
            list(result.partial_conflicts),
            message=result.error
            or (
                "Formatting issues in partially staged files. Auto-fix skipped"
                " to prevent stash conflicts."
            ),
            remediation_cmds=remediation_cmds,
        )

    if not result.success:
        if (
            result.error
            and not result.partial_conflicts
            and not active_reporter.has_errors
        ):
            active_reporter.on_error(result.error)

    active_reporter.finish()
    return 0 if result.success else 1


def run_commit_msg_hook(
    msg_file_path: str | Path,
    *,
    repo_dir: Path | None = None,
    checker_fn: Callable[
        [HookContext, Sequence[str]], bool
    ] = commit_msg_action,
    reporter: ConsoleReporter | None = None,
    is_agent: bool | None = None,
) -> int:
    """Executes commit-msg hook checks on the target commit message file."""
    if _is_env_truthy("FUCHSIA_SKIP_HOOKS"):
        return 0

    if is_agent is None:
        is_agent = is_invoked_by_agent()

    active_reporter = reporter or ConsoleReporter()

    target_dir = repo_dir if repo_dir is not None else Path.cwd()
    try:
        repo_root = paths.get_repo_root(target_dir)
    except RuntimeError as e:
        active_reporter.on_error(str(e))
        active_reporter.finish()
        return 1

    msg_path = Path(msg_file_path)
    msg_file = (
        msg_path if msg_path.is_absolute() else (repo_root / msg_path)
    ).resolve()

    if not msg_file.is_file():
        active_reporter.on_error(f"Commit message file not found: {msg_file}")
        active_reporter.finish()
        return 1

    context = HookContext(
        repo_root=repo_root,
        check_only=False,
        is_agent=is_agent,
        reporter=active_reporter,
    )

    ok = checker_fn(context, [str(msg_file)])

    if not ok:
        if not active_reporter.has_errors:
            active_reporter.on_error("Commit message check failed.")

    active_reporter.finish()
    return 0 if ok else 1


def _build_parser() -> argparse.ArgumentParser:
    """Constructs the standard ArgumentParser for Git hooks CLI dispatcher."""
    parser = argparse.ArgumentParser(
        prog="runner.py",
        description="Fuchsia Git hook CLI runner and hook dispatcher.",
    )

    subparsers = parser.add_subparsers(
        dest="subcommand",
        title="subcommands",
        description="Supported Git hooks",
        required=True,
    )

    subparsers.add_parser(
        "pre-commit",
        help="Run pre-commit hook checks.",
    )

    commit_msg_parser = subparsers.add_parser(
        "commit-msg",
        help="Run commit-msg hook checks.",
    )
    commit_msg_parser.add_argument(
        "msg_file",
        help="Path to commit message file.",
    )

    return parser


def main(raw_args: Sequence[str] | None = None) -> int:
    """Main CLI entrypoint for Git hook dispatcher."""
    if raw_args is None:
        raw_args = sys.argv[1:]

    is_agent = is_invoked_by_agent()

    parser = _build_parser()

    try:
        args = parser.parse_args(raw_args)
    except SystemExit as e:
        return (
            int(e.code)
            if isinstance(e.code, int)
            else (0 if e.code is None else 2)
        )

    reporter = ConsoleReporter()

    if args.subcommand == "pre-commit":
        return run_pre_commit_hook(reporter=reporter, is_agent=is_agent)
    else:
        return run_commit_msg_hook(
            msg_file_path=args.msg_file,
            reporter=reporter,
            is_agent=is_agent,
        )


if __name__ == "__main__":
    sys.exit(main())
