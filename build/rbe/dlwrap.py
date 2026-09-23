#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Download some remote artifacts before running a local command.

Usage:
  $0 [options...] -- command...
  $0 [options...] && ...
"""

import argparse
import os
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

import cl_utils
import fuchsia
import remote_action
import remotetool

_SCRIPT_BASENAME = Path(__file__).name

_PROJECT_ROOT = fuchsia.project_root_dir()

# Needs to be computed with os.path.relpath instead of Path.relative_to
# to support testing a fake (test-only) value of PROJECT_ROOT.
_PROJECT_ROOT_REL = cl_utils.relpath(_PROJECT_ROOT, start=Path(os.curdir))


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def vmsg(verbose: bool, text: str) -> None:
    if verbose:
        msg(text)


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Download from stubs and run a local command.",
        argument_default=None,
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        default=False,
        help="Show what is happening",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        default=False,
        help="Do not download or run the command.",
    )
    parser.add_argument(
        "--undownload",
        action="store_true",
        default=False,
        help="Restore download stubs, if they exist, and do not run the command.",
    )
    parser.add_argument(
        "--download",
        default=[],
        type=Path,
        nargs="*",
        help="Download these files from their stubs.  Arguments are download stub files produced from 'remote_action.py', relative to the working dir.",
    )
    parser.add_argument(
        "--download_list",
        default=[],
        type=Path,
        nargs="*",
        help="Download these files named in these list files.  Arguments are download stub files produced from 'remote_action.py', relative to the working dir.",
    )
    # Positional args are the command and arguments to run.
    parser.add_argument(
        "command", nargs="*", help="The command to run remotely"
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def download_artifacts(
    stub_paths: Sequence[Path],
    downloader: remotetool.RemoteTool,
    working_dir_abs: Path,
    verbose: bool = False,
    dry_run: bool = False,
) -> int:
    """Download remotely stored artifacts.

    Args:
      stub_paths: paths that point to either download stubs or real artifacts.
        Real artifacts are ignored automatically.
    """
    # The download_input_* variant is needed because in this script
    # we are not guaranteed exclusive access to stubs, so downloads
    # must be guarded by locking for mutual exclusion.
    download_statuses = remote_action.download_input_stub_paths_batch(
        downloader=downloader,
        stub_paths=stub_paths,
        working_dir_abs=working_dir_abs,
        verbose=verbose,
    )

    final_status = 0
    for path, status in download_statuses.items():
        if status.returncode != 0:
            final_status = status.returncode
            msg(f"Error downloading {path}.  stderr was:\n{status.stderr_text}")

    if final_status != 0:
        msg("At least one download failed.")

    return final_status


def _main(
    argv: Sequence[str],
    downloader: remotetool.RemoteTool,
    working_dir_abs: Path,
) -> int:
    main_args = _MAIN_ARG_PARSER.parse_args(argv)

    paths = set(main_args.download)
    paths.update(cl_utils.expand_paths_from_files(main_args.download_list))

    if main_args.undownload:
        vmsg(main_args.verbose, f"Restoring download stubs.")
        for p in paths:
            # Fast, no need to parallelize.
            remote_action.undownload(p)

        if main_args.command:
            msg("Not running command, due to --undownload.")
            return 0

        return 0

    # Download artifacts from their stubs.
    status = download_artifacts(
        stub_paths=list(paths),
        downloader=downloader,
        working_dir_abs=working_dir_abs,
        verbose=main_args.verbose,
        dry_run=main_args.dry_run,
    )

    if status != 0:
        return status

    if main_args.command:
        if main_args.dry_run:
            msg("Stopping, due to --dry-run.")
            return 0

        return subprocess.call(main_args.command)

    return 0


def main(argv: Sequence[str]) -> int:
    cfg = _PROJECT_ROOT_REL / remote_action._REPROXY_CFG
    downloader = remotetool.configure_remotetool(cfg)
    return _main(
        argv,
        downloader=downloader,
        working_dir_abs=Path(os.curdir).absolute(),
    )


if __name__ == "__main__":
    remote_action.init_from_main_once()
    sys.exit(main(sys.argv[1:]))
