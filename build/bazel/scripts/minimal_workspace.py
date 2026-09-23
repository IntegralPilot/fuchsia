#!/usr/bin/env python3
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Update or generate Bazel workspace for a minimalist build.

This script is intended to be run outside of any build workflow,
for the purposes of testing bazel features and capabilities
such as remote execution.  This workflow should remain host-agnostic,
and toolchain-agnostic.

This does not require any Fuchsia SDK.

The TOPDIR directory argument will be populated with the following
files:

  $TOPDIR/
    workspace/              Bazel workspace directory.

The workspace/ sub-directory will be populated with:
  * a generated `.bazelrc`
"""

import argparse
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import build_utils
import remote_services_utils

_THIS_SCRIPT = Path(__file__)
_SCRIPT_DIR = _THIS_SCRIPT.resolve().parent


def _main_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawTextHelpFormatter
    )
    parser.add_argument(
        "--fuchsia-dir",
        help="Path to the Fuchsia source tree, auto-detected by default.",
        type=Path,
    )
    parser.add_argument(
        "--bazel-bin",
        help="Path to bazel binary, defaults to $FUCHSIA_DIR/prebuilt/third_party/bazel/${host_platform}/bazel",
        type=Path,
    )
    parser.add_argument(
        "--topdir",
        help="Top output directory. Defaults to out/_bazel_rbe_test",
        type=Path,
    )
    parser.add_argument(
        "--verbose", action="count", default=1, help="Increase verbosity"
    )
    parser.add_argument(
        "--quiet", action="count", default=0, help="Reduce verbosity"
    )
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def force_symlink(src: Path, dest: Path) -> None:
    if src.exists() or src.is_symlink():
        src.unlink()
    src.symlink_to(dest)


def write_file(dest: Path, content: str) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(content)


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args()
    verbosity = args.verbose - args.quiet

    if args.fuchsia_dir:
        fuchsia_dir = args.fuchsia_dir
    else:
        # Assume this script is in 'build/bazel/scripts/'
        # //tools/devshell/rbe always sets this argument,
        # this feature is a convenience when calling this script manually.
        fuchsia_dir = _SCRIPT_DIR.parent.parent.parent

    if args.topdir:
        topdir = args.topdir.resolve()
    else:
        topdir = fuchsia_dir / "out" / "_bazel_rbe_test"

    logs_dir = topdir / "logs"

    host_tag = build_utils.get_host_tag()

    workspace_dir = topdir / "workspace"
    workspace_dir.mkdir(parents=True, exist_ok=True)

    if args.bazel_bin:
        bazel_bin = args.bazel_bin
    else:
        bazel_bin = (
            fuchsia_dir
            / "prebuilt"
            / "third_party"
            / "bazel"
            / host_tag
            / "bazel"
        )

    def log(message: str, level: int = 1) -> None:
        if verbosity >= level:
            print(message)

    def log2(message: str) -> None:
        log(message, 2)

    log2(
        f"""Using directories and files:
  Fuchsia checkout:     {fuchsia_dir}
  Bazel binary:         {bazel_bin}
  Topdir:               {topdir}
  Logs directory:       {logs_dir}
  Bazel workspace:      {workspace_dir}
"""
    )

    # Create bazel symlink
    force_symlink(topdir / "bazel", bazel_bin)

    # top-level symlink to //build/...
    # We only need a small subset of subdirs under //build.
    force_symlink(workspace_dir / "build", fuchsia_dir / "build")

    # symlink to vendored //third_party/bazel_skylib
    (workspace_dir / "third_party").mkdir(parents=True, exist_ok=True)
    force_symlink(
        workspace_dir / "third_party" / "bazel_skylib",
        fuchsia_dir / "third_party" / "bazel_vendor/bazel_skylib+",
    )

    # copy/link over build files (no template expansion needed)
    force_symlink(
        workspace_dir / "BUILD.bazel",
        _SCRIPT_DIR / "minimal_workspace.BUILD.bazel",
    )
    force_symlink(
        workspace_dir / "LICENSE",
        fuchsia_dir / "LICENSE",
    )
    force_symlink(
        workspace_dir / "MODULE.bazel",
        _SCRIPT_DIR / "minimal_workspace.MODULE.bazel",
    )
    force_symlink(
        workspace_dir / "expect_pwd.txt",
        _SCRIPT_DIR / "expect_pwd.txt",
    )

    # Generate args.json expected by fuchsia_platform_build.bzl
    write_file(workspace_dir / "fuchsia_build_generated" / "args.json", "{}")

    # Generate remote_services.bazelrc
    remote_services_utils.generate_remote_services_bazelrc(
        fuchsia_dir=fuchsia_dir,
        output_path=workspace_dir / "remote_services.bazelrc",
        download_outputs="all",  # does not matter here.
    )

    # Generate the content of .bazelrc
    fuchsia_dir / "build" / "bazel" / "templates"

    def expand_template_file(template_file: Path, **kwargs: Any) -> str:
        return template_file.read_text().format(**kwargs)

    bazelrc_content: str = ""
    # platform excerpt from template.bazelrc
    bazelrc_content += """
build --platforms=//build/bazel/platforms:{default_platform}
build --host_platform=//build/bazel/platforms:{host_platform}
import remote_services.bazelrc
""".format(
        default_platform="common",  # see build/bazel/platforms/BUILD.bazel
        host_platform="host",
    )

    bazelrc_dest = workspace_dir / ".bazelrc"
    bazelrc_dest.write_text(bazelrc_content)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
