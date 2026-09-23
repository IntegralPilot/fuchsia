#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""bb_fetch_rbe_cas

Given a bbid and the path of an intermediate output that was remote-built
using RBE, fetch the blob from the RBE CAS.
Note: permissions are not set, so the caller might need to set executable bits.
"""

import argparse
import os
import sys
from collections.abc import Sequence
from pathlib import Path

import bbtool
import cl_utils
import fuchsia
import remotetool
import reproxy_logs

# This requires python pb2 in build/rbe/proto (generated).

_SCRIPT_BASENAME = Path(__file__).name
_SCRIPT_DIR = Path(__file__).parent

PROJECT_ROOT = fuchsia.project_root_dir()
PROJECT_ROOT_REL = cl_utils.relpath(PROJECT_ROOT, start=os.curdir)

_REPROXY_CFG = _SCRIPT_DIR / "fuchsia-reproxy.cfg"


def msg(text: str):
    print(f"[{_SCRIPT_BASENAME}] {text}")


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Fetch build artifacts from RBE CAS.",
        argument_default=None,
    )
    parser.add_argument(
        "--bb",
        type=Path,
        default=bbtool._BB_TOOL,
        help="Path to 'bb' CLI tool.",
        metavar="PATH",
    )
    parser.add_argument(
        "--cfg",
        type=Path,
        default=_REPROXY_CFG,
        help="Reproxy configuration file.",
        metavar="FILE",
    )

    # Require one of the following:
    input_group = parser.add_mutually_exclusive_group(required=True)
    input_group.add_argument(
        "--bbid",
        type=str,
        help="Buildbucket ID (leading 'b' optional/permitted).  Begin search fore reproxy log here.",
        metavar="ID",
    )
    input_group.add_argument(
        "--reproxy_log",
        type=Path,
        help="Use this reproxy log directly instead of searching from bbid.",
        metavar="LOG",
    )

    parser.add_argument(
        "--path",
        type=Path,
        help="Path of artifact under the build output directory.  If omitted, print the path to the cached reproxy log and exit without downloading anything.",
        default=None,
    )
    parser.add_argument(
        "-o",
        dest="output",
        type=Path,
        default=None,
        help="Local output location to fetch to.  If omitted, use the basename of the --path argument.",
        metavar="PATH",
    )
    parser.add_argument(
        "--verbose",
        default=False,
        action=argparse.BooleanOptionalAction,
        help="Print step details.",
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def download_artifact(
    downloader: remotetool.RemoteTool, digest: str, destination: Path
) -> int:
    if destination.exists():
        destination.unlink()

    dl_result = downloader.download_blob(path=destination, digest=digest)
    if dl_result.returncode != 0:
        msg(f"Error downloading blob from CAS with digest {digest}")
        return dl_result.returncode

    return 0


def fetch_artifact_from_reproxy_log(
    reproxy_log: Path,
    artifact_path: Path,
    cfg: Path,
    output: Path,
    verbose: bool = False,
) -> int:
    digest = reproxy_logs.lookup_output_file_digest(
        log=reproxy_log,
        path=artifact_path,
    )
    if digest is None:
        msg(f"Unable to find output file {artifact_path} in the reproxy log.")
        return 1

    if verbose:
        msg(f"Digest of {artifact_path} is {digest}")

    downloader = remotetool.configure_remotetool(cfg)

    output = output or Path(artifact_path.name)
    exit_code = download_artifact(downloader, digest, output)
    if exit_code != 0:
        return exit_code

    msg(f"Artifact {artifact_path} downloaded to {output}.\n  digest: {digest}")
    return 0


def _main(
    bbpath: Path,
    cfg: Path,
    bbid: str = None,
    reproxy_log: Path = None,
    artifact_path: Path = None,
    output: Path = None,
    verbose: bool = False,
) -> int:
    reproxy_log = reproxy_log or bbtool.fetch_reproxy_log_from_bbid(
        bbpath=bbpath,
        bbid=bbid,
        verbose=verbose,
    )
    if reproxy_log is None:
        return 1

    if artifact_path is None:
        # print the path to the log and exit without downloading anything
        msg(f"reproxy log: {reproxy_log}")
        return 0

    return fetch_artifact_from_reproxy_log(
        reproxy_log=reproxy_log,
        artifact_path=artifact_path,
        cfg=cfg,
        output=output or Path(artifact_path.name),
        verbose=verbose,
    )


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)
    if args.output is not None and args.path is None:
        _MAIN_ARG_PARSER.error("-o requires --path")
        return 1
    try:
        return _main(
            bbpath=args.bb,
            bbid=args.bbid.lstrip("b") if args.bbid else None,
            artifact_path=args.path,
            reproxy_log=args.reproxy_log,
            cfg=args.cfg,
            output=args.output,
            verbose=args.verbose,
        )
    except bbtool.BBError as e:
        msg(f"Error: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
