#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""rpl_tool

General collection of tools that involve both reproxy_logs.py
and remotetool.py.
"""

import argparse
import contextlib
import os
import sys
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path

import cl_utils
import fuchsia
import remotetool
import reproxy_logs

# This requires python pb2 in build/rbe/proto (generated).
from api.log import log_pb2

_SCRIPT_BASENAME = Path(__file__).name
_SCRIPT_DIR = Path(__file__).parent

PROJECT_ROOT = fuchsia.project_root_dir()
PROJECT_ROOT_REL = cl_utils.relpath(PROJECT_ROOT, start=os.curdir)

_REPROXY_CFG = _SCRIPT_DIR / "fuchsia-reproxy.cfg"


def msg(text: str):
    print(f"[{_SCRIPT_BASENAME}] {text}")


# Abstract representation of files in the working dir or remote working dir,
# which can be a subdir of exec_root.
_WORKING_DIR_SYMBOL = "${working_directory}"


def normalize_input_path_prefix(path: Path, working_dirs: Iterable[str]) -> str:
    """Substitutes the beginning of path with a symbolic value.

    Args:
      path: file or directory path (relative to exec_root).
      working_dirs: set of path prefixes to normalize.
    """
    path_str = str(path)
    for working_dir in working_dirs:
        if working_dir and path_str.startswith(working_dir):
            return path_str.replace(working_dir, _WORKING_DIR_SYMBOL)
    return path_str  # unmodified


def infer_record_command_and_inputs(
    record: log_pb2.LogRecord, rtool: remotetool.RemoteTool
):
    """Adds command and inputs to a reduced reproxy log record.

    Args:
      record: api.log.LogRecord proto from reproxy (modify-by-reference)
      rtool: remotetool instance for retrieving action details.
    """
    action_digest = record.remote_metadata.action_digest
    # Reconstruct the missing action details (command, inputs)
    # from `remotetool --operation show_action`.
    show_action_details = rtool.show_action(action_digest)
    working_dirs = [
        record.command.working_directory,
        record.command.remote_working_directory,
    ]
    record.command.args.extend(show_action_details.command)
    record.command.input.inputs.extend(
        sorted(
            normalize_input_path_prefix(path, working_dirs)
            for path in show_action_details.inputs.keys()
        )
    )


def expand_to_rpl(
    logdump: log_pb2.LogDump,
    rtool: remotetool.RemoteTool,
    action: Callable[[log_pb2.LogRecord], None] = None,
) -> log_pb2.LogDump:
    """Expands .rrpl to .rpl, adding data for command and inputs.

    Args:
      logdump: api.log.LogDump proto from reproxy (modify-by-reference).
      rtool: remotetool instance for retrieving action details.
      action: action to run on each processed record.

    Returns:
      logdump, modified.
    """
    for record in logdump.records:
        if record.HasField("remote_metadata"):
            infer_record_command_and_inputs(record, rtool)
        if action:
            action(record)
    return logdump


def expand_to_rpl_command(args: argparse.Namespace) -> int:
    """Expands .rrpl to .rpl, adding data for command and inputs.

    This conforms to the interface required for an
    argparse subcommand function.

    Args:
      args: argparse parameters.

    Returns:
      exit code, 0 for success.
    """
    rrpl = reproxy_logs.parse_log(
        log_path=args.rrpl,
        reclient_bindir=fuchsia.RECLIENT_BINDIR,
        verbose=False,
    )
    rtool = remotetool.configure_remotetool(args.cfg)

    # Stream records out to avoid losing data in the event of an error.
    if args.output:
        outf = open(args.output, "w")

        def printer(record: log_pb2.LogRecord):
            outf.write(str(record) + "\n")

    else:
        outf = contextlib.nullcontext()

        def printer(record: log_pb2.LogRecord):
            print(str(record) + "\n")  # print to stdout

    with outf:
        expand_to_rpl(rrpl.proto, rtool, action=printer)

    return 0


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Fetch build artifacts from RBE CAS.",
        argument_default=None,
    )
    subparsers = parser.add_subparsers(required=True)

    expand_parser = subparsers.add_parser(
        "expand_to_rpl", help="Convert .rrpl to .rpl"
    )
    expand_parser.set_defaults(func=expand_to_rpl_command)
    expand_parser.add_argument(
        "rrpl",
        type=Path,
        help="Reduced reproxy log (.rrpl)",
        metavar="LOG",
        default=None,
    )
    expand_parser.add_argument(
        "--cfg",
        type=Path,
        help="reproxy config",
        default=_REPROXY_CFG,
    )
    expand_parser.add_argument(
        "-o",
        type=Path,
        dest="output",
        help="output to write .rpl",
        default=None,
    )

    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
