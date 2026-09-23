#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia Standalone Performance Analysis Tool.

This tool provides CLI commands to query diagnostic plugins and analyze performance.
"""

import argparse
import logging
import sys
from typing import Sequence

import result_formatter
from binder import BinderPlugin
from cpu import CpuPlugin
from plugins import AnalyzePlugin, PluginArgumentError
from query import TpShell


def main(
    argv: Sequence[str] | None = None,
    plugins: Sequence[AnalyzePlugin] | None = None,
) -> int:
    """Main entry point for the performance analysis tool.

    Args:
        argv: Optional list of arguments to parse. Defaults to sys.argv[1:].
        plugins: Optional sequence of AnalyzePlugin instances to register.

    Returns:
        Exit code (0 for success, non-zero for failure).
    """
    parser = argparse.ArgumentParser(
        description="Fuchsia Standalone Performance Analysis Tool",
        exit_on_error=False,
    )
    parser.add_argument(
        "--format",
        choices=["json", "markdown", "text"],
        default="text",
        help="Output format (default: text)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Enable verbose debug logging",
    )
    parser.add_argument(
        "--no-cache",
        action="store_false",
        dest="cache",
        help="Disable local caching of permalink URL traces",
    )
    subparsers = parser.add_subparsers(dest="command")

    # 'query' subcommand
    query_parser = subparsers.add_parser(
        "query",
        help="Perform SQL queries on a trace file",
        exit_on_error=False,
    )
    query_parser.add_argument(
        "--trace", required=True, help="Trace file path or URL"
    )
    group = query_parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--sql", help="SQL query to run")
    group.add_argument("--batch", help="Batch JSON or @file")

    # 'analyze' subcommand
    analyze_parser = subparsers.add_parser(
        "analyze",
        help="Analyze traces with high-level plugins",
        add_help=False,
        exit_on_error=False,
    )
    analyze_parser.add_argument(
        "-h",
        "--help",
        action="store_true",
        help="show this help message and exit",
    )
    analyze_parser.add_argument("--trace", help="Trace file path or URL")
    analyze_parser.add_argument("--plugin", help="Plugin to run")
    analyze_parser.add_argument(
        "--list-plugins", action="store_true", help="List available plugins"
    )

    try:
        args, remaining_args = parser.parse_known_args(argv)
    except argparse.ArgumentError as e:
        parser.print_usage(sys.stderr)
        print(f"{parser.prog}: error: {e}", file=sys.stderr)
        return 2
    except SystemExit as e:
        return e.code if isinstance(e.code, int) else 0

    # Configure logging based on verbose flag
    log_level = logging.DEBUG if args.verbose else logging.WARNING
    logging.basicConfig(
        level=log_level,
        format="%(levelname)s:%(name)s:%(message)s",
        stream=sys.stderr,
    )

    formatter = result_formatter.FORMATTERS[args.format]

    if args.command is None:
        parser.print_help()
        return 2

    if args.command == "query":
        if remaining_args:
            query_parser.print_usage(sys.stderr)
            print(
                f"{query_parser.prog}: error: unrecognized arguments: {' '.join(remaining_args)}",
                file=sys.stderr,
            )
            return 2

        tp_shell = TpShell()
        try:
            if args.sql:
                raw_results = tp_shell.query(
                    trace_path=args.trace,
                    sql=args.sql,
                    cache=args.cache,
                )
                print(formatter.format_raw_data(raw_results))
            else:
                batch_results = tp_shell.query(
                    trace_path=args.trace,
                    batch=args.batch,
                    cache=args.cache,
                )
                print(formatter.format_results(batch_results))
            return 0
        except Exception as e:
            print(formatter.format_error(str(e)))
            return 1

    elif args.command == "analyze":
        if plugins is None:
            plugins = [BinderPlugin(), CpuPlugin()]

        if args.list_plugins:
            plugin_data = [
                {"name": p.name, "description": p.description} for p in plugins
            ]
            print(formatter.format_raw_data(plugin_data))
            return 0

        if args.help:
            if args.plugin:
                plugin = next(
                    (p for p in plugins if p.name == args.plugin), None
                )
                if not plugin:
                    print(
                        f"Error: Plugin '{args.plugin}' not found.",
                        file=sys.stderr,
                    )
                    return 2
                try:
                    plugin.analyze(
                        ["--help"], args.trace or "", cache=args.cache
                    )
                    return 0
                except SystemExit as e:
                    if e.code is None or e.code == 0:
                        return 0
                    return e.code if isinstance(e.code, int) else 1
                except Exception as e:
                    print(f"Error: {e}", file=sys.stderr)
                    return 1
            else:
                analyze_parser.print_help()
                return 0

        if not args.trace or not args.plugin:
            missing = []
            if not args.trace:
                missing.append("--trace")
            if not args.plugin:
                missing.append("--plugin")
            analyze_parser.print_usage(sys.stderr)
            print(
                f"{analyze_parser.prog}: error: the following arguments are required: {', '.join(missing)}",
                file=sys.stderr,
            )
            return 2

        plugin = next((p for p in plugins if p.name == args.plugin), None)
        if not plugin:
            print(f"Error: Plugin '{args.plugin}' not found.", file=sys.stderr)
            return 2

        try:
            analysis_results = plugin.analyze(
                remaining_args, args.trace, cache=args.cache
            )
            print(formatter.format_results(analysis_results))
            return 0
        except PluginArgumentError as e:
            print(formatter.format_error(str(e)))
            return 2
        except SystemExit as e:
            if e.code is None or e.code == 0:
                return 0
            return e.code if isinstance(e.code, int) else 1
        except Exception as e:
            print(formatter.format_error(str(e)))
            return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
