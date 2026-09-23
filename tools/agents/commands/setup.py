# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Setup subcommand handler for configuring AI coding agent permissions."""

from __future__ import annotations

import argparse
import pathlib
import sys

from agents.lib import (
    config,
    githooks,
    paths,
    permissions,
    services,
    state,
)

DEFAULT_PROFILE = "local-changes"


def register_subcommand(
    subparsers: argparse._SubParsersAction[argparse.ArgumentParser],
) -> argparse.ArgumentParser:
    """Register the setup subcommand with the top-level argument parser."""
    parser = subparsers.add_parser(
        "setup",
        help="Configure AI coding agent permissions and profiles.",
        description="Configure AI coding agent permissions and profiles.",
    )
    add_arguments(parser)
    parser.set_defaults(func=run)
    return parser


def add_arguments(parser: argparse.ArgumentParser) -> None:
    """Add arguments for setup command to parser."""
    parser.add_argument(
        "-p",
        "--profile",
        choices=list(permissions.PROFILE_DEFINITIONS.keys()),
        help="Permission profile flavor (read-only, local-changes, external-changes, full-access). Defaults to local-changes.",
    )
    mode_group = parser.add_mutually_exclusive_group()
    mode_group.add_argument(
        "--status",
        action="store_true",
        help="Display current agent configuration status, active profile, and rule counts.",
    )
    mode_group.add_argument(
        "--rollback",
        nargs="?",
        const=1,
        type=int,
        default=None,
        metavar="N",
        help="Roll back configuration to N setups ago (default: 1).",
    )
    mode_group.add_argument(
        "--reset",
        action="store_true",
        help="Purge all Fuchsia-managed rules from configuration while preserving user custom rules.",
    )
    parser.add_argument(
        "--state-dir",
        type=pathlib.Path,
        default=None,
        help="Custom path to state directory (default: ~/.local/share/Fuchsia/agents/setup).",
    )
    parser.add_argument(
        "-a",
        "--allow",
        action="append",
        default=[],
        help="Additional grant rule to ALLOW",
    )
    parser.add_argument(
        "-d",
        "--deny",
        action="append",
        default=[],
        help="Additional grant rule to DENY",
    )
    parser.add_argument(
        "-k",
        "--ask",
        action="append",
        default=[],
        help="Additional grant rule to ASK",
    )
    parser.add_argument(
        "--allow-list",
        action="append",
        default=[],
        type=pathlib.Path,
        help="Extra allowed list file",
    )
    parser.add_argument(
        "--deny-list",
        action="append",
        default=[],
        type=pathlib.Path,
        help="Extra denied list file",
    )
    parser.add_argument(
        "--ask-list",
        action="append",
        default=[],
        type=pathlib.Path,
        help="Extra ask list file",
    )
    parser.add_argument(
        "--config",
        type=pathlib.Path,
        default=None,
        help="Custom path to output Gemini configuration file (default: ~/.gemini/config/config.json)",
    )
    parser.add_argument(
        "--git-hooks",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Manage and sync Git hooks across checkout repositories during both initial setup and reset/uninstall workflows (default: True).",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print changes without modifying config or services.",
    )


def _restart_daemons(fuchsia_dir: pathlib.Path, dry_run: bool) -> None:
    """Discover and restart managed agent daemon services."""
    daemon_services = services.find_daemon_services(fuchsia_dir)
    services.restart_daemons(
        service_names=daemon_services,
        dry_run=dry_run,
    )


def _format_hook_count(count: int) -> str:
    return f"{count} Git hook{'s' if count != 1 else ''}"


def _report_hook_results(
    count: int,
    failed: list[tuple[pathlib.Path, str]],
    action_verb: str,
    past_verb: str,
    dry_run: bool,
    has_repos: bool = True,
) -> None:
    prefix = "[DRY RUN] " if dry_run else ""
    if count:
        verb = f"Would {action_verb}" if dry_run else past_verb
        msg = _format_hook_count(count)
        print(f"{prefix}{verb} {msg} across checkout.")
    elif not has_repos:
        print(f"{prefix}No Git repositories found across checkout.")
    elif not failed:
        if action_verb == "remove":
            print(f"{prefix}No Git hooks found to remove.")
        else:
            print(f"{prefix}No Git hooks configured.")
    if failed:
        for repo, err in failed:
            print(
                f"Warning: Failed to {action_verb} Git hooks in {repo}: {err}",
                file=sys.stderr,
            )


def run(args: argparse.Namespace) -> int:
    """Execute setup with parsed arguments."""
    fuchsia_dir = paths.find_fuchsia_dir()
    state_dir = args.state_dir or state.get_default_state_dir()
    config_path = args.config or config.get_default_config_path()

    if args.status:
        status_text = state.format_status(
            config_path=config_path,
            state_path=state_dir / "state.json",
        )
        print(status_text)
        hook_status = githooks.get_git_hooks_status(fuchsia_dir)
        repo_label = (
            "repository" if hook_status.total_repos == 1 else "repositories"
        )
        print(
            f"Git hooks: Configured across {hook_status.configured_repos}/{hook_status.total_repos} {repo_label}."
        )
        return 0

    if args.rollback is not None:
        steps = args.rollback
        success = state.rollback(
            config_path=config_path,
            state_path=state_dir / "state.json",
            backups_dir=state_dir / "backups",
            steps=steps,
            dry_run=args.dry_run,
        )
        if success:
            _restart_daemons(fuchsia_dir, args.dry_run)
        return 0 if success else 1

    if args.reset:
        success = state.reset(
            config_path=config_path,
            state_path=state_dir / "state.json",
            backups_dir=state_dir / "backups",
            dry_run=args.dry_run,
        )
        if success:
            if args.git_hooks:
                uninstalled = githooks.uninstall_git_hooks(
                    fuchsia_dir, dry_run=args.dry_run
                )
                _report_hook_results(
                    count=len(uninstalled.modified),
                    failed=uninstalled.failed,
                    action_verb="remove",
                    past_verb="Removed",
                    dry_run=args.dry_run,
                    has_repos=bool(paths.find_checkout_git_repos(fuchsia_dir)),
                )
            _restart_daemons(fuchsia_dir, args.dry_run)
        return 0 if success else 1

    journal = state.load_state(state_dir / "state.json")

    selected_profile = args.profile
    has_explicit_rules = bool(
        args.allow
        or args.deny
        or args.ask
        or args.allow_list
        or args.deny_list
        or args.ask_list
    )

    if not selected_profile:
        if journal.active_profile:
            selected_profile = journal.active_profile
        elif not has_explicit_rules:
            selected_profile = DEFAULT_PROFILE

    allow_grants: list[str] = []
    deny_grants: list[str] = []
    ask_grants: list[str] = []

    if selected_profile:
        print(f"\nApplying permission profile: [{selected_profile}]")
        grants = permissions.load_profile_grants(fuchsia_dir, selected_profile)
        allow_grants.extend(grants.allow)
        deny_grants.extend(grants.deny)
        ask_grants.extend(grants.ask)

    for raw_cmds, list_files, grants_target in (
        (args.allow, args.allow_list, allow_grants),
        (args.deny, args.deny_list, deny_grants),
        (args.ask, args.ask_list, ask_grants),
    ):
        for cmd in raw_cmds:
            grants_target.extend(permissions.expand_command_variants(cmd))
        for f in list_files:
            grants_target.extend(permissions.read_command_list_file(f))

    success = config.apply_grants(
        config_path=config_path,
        allow=allow_grants,
        deny=deny_grants,
        ask=ask_grants,
        dry_run=args.dry_run,
        state_dir=state_dir,
        selected_profile=selected_profile or "",
    )
    if not success:
        return 1

    if args.git_hooks:
        installed = githooks.install_git_hooks(
            fuchsia_dir, dry_run=args.dry_run
        )
        _report_hook_results(
            count=len(installed.modified),
            failed=installed.failed,
            action_verb="configure",
            past_verb="Configured",
            dry_run=args.dry_run,
            has_repos=bool(paths.find_checkout_git_repos(fuchsia_dir)),
        )

    _restart_daemons(fuchsia_dir, args.dry_run)
    return 0
