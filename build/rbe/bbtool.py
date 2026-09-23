#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""bbtool

Collection of 'bb' (buildbucket) utilities for working with infra builds.
See subcommands.
"""

import argparse
import json
import os
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import cas
import cl_utils
import fuchsia

_SCRIPT_BASENAME = Path(__file__).name
_SCRIPT_DIR = Path(__file__).parent

PROJECT_ROOT = fuchsia.project_root_dir()
PROJECT_ROOT_REL = cl_utils.relpath(PROJECT_ROOT, Path(start=os.curdir))

# default path to `bb` buildbucket tool
_BB_TOOL = PROJECT_ROOT_REL / "prebuilt" / "tools" / "buildbucket" / "bb"


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def log_cache_path(build_id: str, log_name: str) -> Path:
    """Returns the path use for locally caching reproxy logs."""
    tempdir = Path(tempfile.gettempdir())  # e.g. /tmp
    return (
        tempdir
        / _SCRIPT_BASENAME
        / "reproxy_logs_cache"
        / f"b{build_id}"
        / log_name
    )


class BBError(RuntimeError):
    """BuildBucketTool related errors."""

    def __init__(self, msg: str):
        super().__init__(msg)


class BuildBucketTool(object):
    def __init__(self, bb: Path | None = None):
        self.bb = bb or _BB_TOOL

    def get_json_fields(self, bbid: str) -> dict[str, Any]:
        bb_result = cl_utils.subprocess_call(
            [
                str(self.bb),
                "get",
                bbid,
                "--json",
                "--fields",
                "output.properties",
            ],
            quiet=True,
        )
        if bb_result.returncode != 0:
            for line in bb_result.stderr:
                print(line)
            raise BBError(f"bb failed to lookup id {bbid}.")
        return json.loads("\n".join(bb_result.stdout) + "\n")

    def download_reproxy_log(self, build_id: str, reproxy_log_name: str) -> str:
        # 'bb log' prints log contents to stdout.  Capture it and write it out.
        rpl_log_result = cl_utils.subprocess_call(
            [
                str(self.bb),
                "log",
                build_id,
                f"build|teardown remote execution|read {reproxy_log_name}",
                reproxy_log_name,
            ],
            quiet=True,
        )
        if rpl_log_result.returncode != 0:
            for line in rpl_log_result.stderr:
                print(line)
            raise BBError(f"Failed to fetch bb reproxy log {reproxy_log_name}.")
        return "\n".join(rpl_log_result.stdout) + "\n"

    def get_rbe_build_info(
        self, bbid: str, verbose: bool = False
    ) -> tuple[str, dict[str, Any]]:
        """Returns info for the build that actually used RBE (maybe a subbuild).

        Args:
          bbid: build id
          verbose: if true, print what is happening

        Returns:
          1) build id
          2) output properties as JSON object
        """
        if verbose:
            msg(f"Looking up output properties of build {bbid}")
        bb_json = self.get_json_fields(bbid)

        if verbose:
            msg(f"Checking for child build of {bbid}")

        output_properties = bb_json["output"]["properties"]
        child_build_id = output_properties.get("child_build_id")

        # `rbe_build_id` points to the build that contains
        # the reproxy log with information about a remote built
        # artifact.
        if child_build_id:
            rbe_build_id = child_build_id
            rbe_build_json = self.get_json_fields(rbe_build_id)
        else:
            # re-use bb_json
            rbe_build_id = bbid
            rbe_build_json = bb_json

        return rbe_build_id, rbe_build_json


def _cache_path_exists(path: Path) -> bool:
    """This is defined for the sake of mocking in tests."""
    return path.exists()


def fetch_reproxy_log_from_bbid(
    bbpath: Path, bbid: str, verbose: bool = False
) -> Path | None:
    bb = BuildBucketTool(bbpath)

    # Get the build that actually used RBE, and has an reproxy log.
    rbe_build_id, rbe_build_json = bb.get_rbe_build_info(bbid, verbose=verbose)
    if verbose:
        msg(f"Using build id {rbe_build_id} to look for reproxy logs")

    output_properties = rbe_build_json["output"]["properties"]

    # The current way to get reproxy logs is from CAS (uploaded by recipe).
    rpl_params = output_properties.get("cas_reproxy_log")

    if rpl_params:
        reproxy_log_name = rpl_params["file"]

    else:
        # legacy support: older infra builds published reproxy logs to LogDog
        rpl_files = rbe_build_json["output"]["properties"].get("rpl_files")

        if rpl_files is None:
            msg(f"Error looking up reproxy log from build {rbe_build_id}")
            return None

        # Assume there is only one.
        reproxy_log_name = rpl_files[0]

    # Check if log already exists in local download cache.
    reproxy_log_cache_path = log_cache_path(rbe_build_id, reproxy_log_name)
    reproxy_log_cache_path.parent.mkdir(parents=True, exist_ok=True)
    if _cache_path_exists(reproxy_log_cache_path):
        if verbose:
            msg(f"Re-using cached reproxy log at {reproxy_log_cache_path}")

        return reproxy_log_cache_path

    # else not already in cache, so download and cache it.
    if verbose:
        msg(
            f"Downloading reproxy log {reproxy_log_name}.  (This could take a few minutes.)"
        )

    if rpl_params:
        file = cas.File(
            instance=rpl_params["instance"],
            digest=rpl_params["digest"],
            filename=reproxy_log_name,
        )
        dl_result = file.download(reproxy_log_cache_path)
        if dl_result.returncode != 0:
            # An error is already printed.
            return None

    else:
        rpl_log_contents = bb.download_reproxy_log(
            rbe_build_id, reproxy_log_name
        )
        reproxy_log_cache_path.write_text(rpl_log_contents)

    if verbose:
        msg(f"Reproxy log cached to {reproxy_log_cache_path}")

    # TODO: writing log out to disk and re-reading/parsing it can be slow.
    # Perhaps add an interface to take a string in-memory.
    return reproxy_log_cache_path


def fetch_reproxy_log_command(args: argparse.Namespace) -> int:
    try:
        log_path = fetch_reproxy_log_from_bbid(
            bbpath=args.bb, bbid=args.bbid, verbose=args.verbose
        )
    except BBError as e:
        msg(f"Error: {e}")
        return 1
    msg(f"reproxy log name: {log_path}")
    return 0


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Perform an operation using 'bb'.",
        argument_default=None,
    )
    parser.add_argument(
        "--bb",
        type=Path,
        default=_BB_TOOL,
        help="Path to 'bb' CLI tool.",
        metavar="PATH",
    )
    parser.add_argument(
        "--bbid",
        type=str,
        help="Buildbucket ID (leading 'b' optional/permitted).",
        metavar="ID",
        required=True,
    )
    parser.add_argument(
        "--verbose",
        default=False,
        action=argparse.BooleanOptionalAction,
        help="Print step details.",
    )

    subparsers = parser.add_subparsers(required=True)

    reproxy_log_fetcher = subparsers.add_parser(
        "fetch_reproxy_log", help="Download the reproxy log"
    )
    reproxy_log_fetcher.set_defaults(func=fetch_reproxy_log_command)

    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)
    args.bbid = args.bbid.lstrip("b")
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
