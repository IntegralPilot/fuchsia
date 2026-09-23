#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Common wrapper for running prebuilt tools remotely.

This script functions as a standalone executable.

Usage:
  $0 [remote options...] -- command...
"""

import argparse
import os
import subprocess
import sys
from collections.abc import Iterable, Sequence
from pathlib import Path
from typing import Any

import cl_utils
import fuchsia
import remote_action

_SCRIPT_BASENAME = Path(__file__).name
_SCRIPT_DIR = Path(__file__).parent


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def _main_arg_parser() -> argparse.ArgumentParser:
    """Construct the argument parser, called by main()."""
    parser = argparse.ArgumentParser(
        description="Prepares a command for remote execution.",
        argument_default=[],
        add_help=True,  # Want this to exit after printing --help
    )
    remote_action.inherit_main_arg_parser_flags(parser)
    parser.add_argument(
        "--label_toolname",  # "toolname" is a special label for rewrapper
        type=Path,
        default=None,
        metavar="toolpath",
        help="Identifies the action type for metrics purposes.  The basename of this tool is forwarded to `rewrapper --label=toolname=NAME`.",
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


class PrebuiltToolAction(object):
    """Generic remote wrapper for running a tool in Fuchsia's prebuilts."""

    def __init__(
        self,
        argv: Sequence[str],
        exec_root: Path | None = None,
        working_dir: Path | None = None,
        host_platform: str | None = None,
        auto_reproxy: bool = True,  # can disable for unit-testing
    ):
        self._working_dir = (working_dir or Path(os.curdir)).absolute()
        self._exec_root = (exec_root or remote_action.PROJECT_ROOT).absolute()
        self._host_platform = host_platform or fuchsia.HOST_PREBUILT_PLATFORM

        # Propagate --remote-flag=... options to the remote prefix,
        # as if they appeared before '--'.
        # Forwarded rewrapper options with values must be written as '--flag=value',
        # not '--flag value' because argparse doesn't know what unhandled flags
        # expect values.
        main_argv, self._local_command = remote_action.forward_remote_flags(
            argv
        )

        # forward all unknown flags to rewrapper
        # --help here will result in early exit()
        (
            self._main_args,
            self._main_remote_options,
        ) = _MAIN_ARG_PARSER.parse_known_args(main_argv)

        # Re-launch with reproxy if needed.
        if auto_reproxy:
            remote_action.auto_relaunch_with_reproxy(
                script=Path(__file__), argv=argv, args=self._main_args
            )

        if not self.local_command:  # there is no command, bail out early
            return

        self._local_only = self._main_args.local

        self._cleanup_files: Sequence[Path] = []
        self._remote_action = self._setup_remote_action()

    @property
    def local_command(self) -> Sequence[str]:
        return self._local_command

    @property
    def remote_command(self) -> Sequence[str]:
        return [
            str(self.remote_tool) if Path(tok) == self.local_tool else tok
            for tok in self._local_command
        ]

    def check_preconditions(self) -> None:
        # check for required remote tools
        tool = self.remote_tool
        if tool is None or not tool.exists():
            raise RuntimeError(
                f"Missing the following tools for remote execution: {tool}.  See tqr/563535 for how to fetch the needed packages."
            )

    @property
    def command_line_inputs(self) -> Sequence[Path]:
        return [
            Path(p) for p in cl_utils.flatten_comma_list(self._main_args.inputs)
        ]

    @property
    def command_line_inputs_lists(self) -> Sequence[Path]:
        return [
            Path(p)
            for p in cl_utils.flatten_comma_list(
                self._main_args.input_list_paths
            )
        ]

    @property
    def command_line_output_files(self) -> Sequence[Path]:
        return [
            Path(p)
            for p in cl_utils.flatten_comma_list(self._main_args.output_files)
        ]

    @property
    def command_line_output_dirs(self) -> Sequence[Path]:
        return [
            Path(p)
            for p in cl_utils.flatten_comma_list(
                self._main_args.output_directories
            )
        ]

    def prepare(self) -> None:
        """Setup everything ahead of remote execution.

        Raises:
          RuntimeError exception if pre-flight requirements are not met.
        """
        assert (
            not self.local_only
        ), "This should not be reached in local-only mode."

        self.check_preconditions()

    def _setup_remote_action(self) -> remote_action.RemoteAction:
        if self.label_toolname is None:
            raise ValueError("self.label_toolname is None")
        tool_name = self.label_toolname.name
        remote_options = [
            # type=tool says we are providing a custom tool, and thus,
            #   own the logic for providing explicit inputs.
            # shallow=true works around an issue where racing mode downloads
            #   incorrectly
            # toolname self-identifies the action type for metrics
            f"--labels=type=tool,shallow=true,toolname={tool_name}",
            # --canonicalize_working_dir: coerce the output dir to a constant.
            #   This requires that the command be insensitive to output dir, and
            #   that its outputs do not leak the remote output dir.
            #   Ensuring that the results reproduce consistently across different
            #   build directories helps with caching.
            "--canonicalize_working_dir=true",
        ] + self._main_remote_options  # allow forwarded options to override defaults

        # Automatically add the tool.
        remote_inputs = (
            [self.remote_tool] if self.remote_tool is not None else []
        )

        action = remote_action.remote_action_from_args(
            main_args=self._main_args,  # includes inputs and outputs already
            remote_options=remote_options,
            command=self.remote_command,
            inputs=remote_inputs,
            working_dir=self.working_dir,
            exec_root=self.exec_root,
        )
        self.vprintlist("remote inputs", action.inputs_relative_to_project_root)
        self.vprintlist(
            "remote output files", action.output_files_relative_to_project_root
        )
        self.vprintlist(
            "remote output dirs", action.output_dirs_relative_to_project_root
        )
        self.vprintlist("rewrapper options", remote_options)
        return action

    @property
    def remote_action(self) -> remote_action.RemoteAction:
        return self._remote_action

    @property
    def working_dir(self) -> Path:
        return self._working_dir

    @property
    def exec_root(self) -> Path:
        return self._exec_root

    @property
    def exec_root_rel(self) -> Path:
        return cl_utils.relpath(self.exec_root, start=self.working_dir)

    @property
    def host_platform(self) -> str:
        return self._host_platform

    @property
    def verbose(self) -> bool:
        return self._main_args.verbose

    @property
    def dry_run(self) -> bool:
        return self._main_args.dry_run

    def vmsg(self, text: str) -> None:
        if self.verbose:
            msg(text)

    def vprintlist(self, desc: str, items: Iterable[Any]) -> None:
        """In verbose mode, print elements.

        Args:
          desc: text description of what is being printed.
          items: stream of any type of object that is str-able.
        """
        if self.verbose:
            msg(f"{desc}: {{")
            for item in items:
                text = str(item)
                print(f"  {text}")
            print(f"}}  # {desc}")

    @property
    def local_only(self) -> bool:
        return self._local_only

    @property
    def local_tool(self) -> Path | None:
        for tok in self.local_command:
            if "=" not in tok:
                return Path(tok)
        return None

    @property
    def label_toolname(self) -> Path | None:
        toolname = self._main_args.label_toolname
        return (
            toolname
            if (isinstance(toolname, Path) and toolname is not None)
            else self.local_tool
        )

    @property
    def remote_tool(self) -> Path | None:
        # selects the binary for the remote execution platform
        if self.local_tool is None:
            return None
        return fuchsia.remote_executable(self.local_tool)

    def _run_locally(self) -> int:
        return subprocess.call(
            cl_utils.auto_env_prefix_command(list(self.local_command))
        )

    def _run_remote_action(self) -> int:
        return self.remote_action.run_with_main_args(self._main_args)

    def run(self) -> int:
        if self.local_only:
            return self._run_locally()

        self.prepare()

        try:
            return self._run_remote_action()
        finally:
            if not self._main_args.save_temps:
                self._cleanup()

    def _cleanup(self) -> None:
        for f in self._cleanup_files:
            f.unlink()


def main(argv: Sequence[str]) -> int:
    action = PrebuiltToolAction(
        argv,  # [remote options] -- command...
        exec_root=remote_action.PROJECT_ROOT,
        working_dir=Path(os.curdir),
        host_platform=fuchsia.HOST_PREBUILT_PLATFORM,
    )
    return action.run()


if __name__ == "__main__":
    remote_action.init_from_main_once()
    sys.exit(main(sys.argv[1:]))
