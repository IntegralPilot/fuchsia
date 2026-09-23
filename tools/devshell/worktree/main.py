#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import sys

from subcommands import add as add_cmd
from subcommands import list as list_cmd
from subcommands import locate as locate_cmd
from subcommands import pool_add as pool_add_cmd
from subcommands import pool_list as pool_list_cmd
from subcommands import pool_remove as pool_remove_cmd
from subcommands import remove as remove_cmd
from worktree_pool import WorktreePool


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="fx worktree",
        description="Manage Fuchsia worktrees for parallel development.",
        epilog=(
            "Workflow:\n"
            "  • fx worktree add <name>     Create a new worktree.\n"
            "  • fx worktree remove <name>  Remove a worktree.\n"
            "\n"
            "Advanced (Internal Pool Management):\n"
            "  For performance, worktrees are transparently backed by a pool of reusable checkouts.\n"
            "  You can manage this pool using 'fx worktree pool' commands (e.g., 'fx worktree pool list')."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    subparsers = parser.add_subparsers(
        dest="subcommand", required=True, metavar="COMMAND"
    )

    # Subcommand group 'pool'
    parser_pool = subparsers.add_parser(
        "pool",
        description="Administrative commands for provisioning and maintaining physical checkouts on disk.",
    )
    pool_subparsers = parser_pool.add_subparsers(
        dest="pool_subcommand", required=True
    )

    parser_pool_list = pool_subparsers.add_parser(
        "list",
        help="List all physical worktrees in the pool",
        description="List all physical worktrees in the pool along with state and physical paths.",
    )

    parser_pool_add = pool_subparsers.add_parser(
        "add",
        help="Add a new physical worktree checkout to the pool",
    )
    parser_pool_add.add_argument(
        "name", nargs="?", help="Optional physical name"
    )
    parser_pool_add.add_argument("--set", action="append", help="Run 'fx set'")
    group = parser_pool_add.add_mutually_exclusive_group()
    group.add_argument(
        "--symlink-local",
        action="store_true",
        help="Symlink the 'local' directory from the main checkout",
    )
    group.add_argument(
        "--copy-local",
        action="store_true",
        help="Copy the 'local' directory from the main checkout",
    )

    parser_pool_remove = pool_subparsers.add_parser(
        "remove",
        help="Remove a physical worktree from the pool",
    )
    parser_pool_remove.add_argument("name", help="Name of physical worktree")
    parser_pool_remove.add_argument(
        "--force", action="store_true", help="Force removal"
    )

    # Subcommand 'locate'
    parser_locate = subparsers.add_parser(
        "locate",
        help="Print the absolute path to a worktree directory",
        description="Locate a worktree by name and print its absolute path on disk.",
    )
    parser_locate.add_argument("name", help="Name of the worktree")

    # Subcommand 'list'
    subparsers.add_parser(
        "list",
        help="List all leased worktrees",
        description="List all leased worktrees and their git branches.",
    )

    # Subcommand 'add'
    parser_add = subparsers.add_parser(
        "add",
        help="Add a leased worktree checkout for development",
    )
    parser_add.add_argument("name", help="Name of worktree / branch")
    parser_add.add_argument(
        "--sync", action="store_true", help="Sync after adding"
    )
    parser_add.add_argument(
        "--pool-name", help="Specific pool slot to allocate"
    )
    parser_add.add_argument("--json", action="store_true", help="Output JSON")

    # Subcommand 'remove'
    parser_remove = subparsers.add_parser(
        "remove",
        help="Remove a leased worktree and return it to the pool",
    )
    parser_remove.add_argument("name", help="Name of worktree to remove")

    # Internal subcommand '_complete' for shell completion
    parser_complete = subparsers.add_parser(
        "_complete",
        help="Internal completion helper",
    )
    parser_complete.add_argument(
        "filter",
        choices=[
            "physical_free",
            "physical_leased",
            "physical_all",
            "leased",
            "all",
        ],
    )

    args = parser.parse_args()
    pool = WorktreePool()

    try:
        if args.subcommand == "pool":
            if args.pool_subcommand == "list":
                pool_list_cmd.run(pool)
            elif args.pool_subcommand == "add":
                pool_add_cmd.run(args, pool)
            elif args.pool_subcommand == "remove":
                pool_remove_cmd.run(args, pool)
        elif args.subcommand == "locate":
            locate_cmd.run(args, pool)
        elif args.subcommand == "list":
            list_cmd.run(args, pool)
        elif args.subcommand == "add":
            add_cmd.run(args, pool)
        elif args.subcommand == "remove":
            remove_cmd.run(args, pool)
        elif args.subcommand == "_complete":
            print(" ".join(pool.get_completion_choices(args.filter)))
        else:
            print(f"Unknown subcommand: {args.subcommand}", file=sys.stderr)
            sys.exit(1)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
