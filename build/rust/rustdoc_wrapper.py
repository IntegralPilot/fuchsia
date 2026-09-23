#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""rustdoc_wrapper
Runs rustdoc to document a crate; executed as part of the .rustdoc subtarget.
"""

import json
import re
import shutil
from argparse import ArgumentParser, BooleanOptionalAction, Namespace
from os import walk
from pathlib import Path
from subprocess import run
from sys import argv
from zipfile import ZipFile


def label_to_crate_name(label: str):
    """
    This function is not robust, but in the context of
    //third_party/rust_crates it is fine (ethanws@)
    """
    match = re.match(r"^:(.+?)-v\d+_\d+_\d+\.actual$", label)
    assert (
        match is not None
    ), f"rustdoc_wrapper: the crate name could not be extacted from {label}"
    return match.group(1).replace("-", "_")


def get_aliases(args: Namespace) -> dict[str, str]:
    """
    Returns a map from the original crate name to the new crate name.
    """
    if args.aliased_deps_map is None:
        return dict()
    aliases = json.loads(args.aliased_deps_map.read_text())
    return {label_to_crate_name(v): k for k, v in aliases.items()}


def fixup_extern(extern: str, aliases: dict[str, str]) -> tuple[str, str]:
    """
    Adjusts the crate name in an --extern or --extern-html-root-url
    to respect the alias map.
    """
    crate_name, path = extern.split("=")
    if crate_name in aliases:
        crate_name = aliases[crate_name]
    return crate_name, path


def zip_dir(src: Path, dst: Path):
    """
    Creates a zip archive called `dst`, containing the recursive contents of the `src` directory.
    """
    with ZipFile(dst, mode="w") as dst:
        for dirpath, dirnames, filenames in walk(src):
            dst.mkdir(str(Path(dirpath).relative_to(src)))
            for filename in filenames:
                path = Path(dirpath, filename)
                dst.write(
                    filename=str(path),
                    arcname=str(path.relative_to(src)),
                )


def main(args: Namespace, rustdoc_invocation: list[str]) -> int:
    """Runs rustdoc executable."""
    assert not (
        (args.zip_from is None) ^ (args.zip_to is None)
    ), "must provide either neither or both of --zip-from and --zip-to"

    aliases = get_aliases(args)
    extern = (
        f'--extern={"=".join(fixup_extern(e, aliases))}' for e in args.extern
    )
    extern_html_root_urls = (
        f"--extern-html-root-url={fixup_extern(e, aliases)[0]}={args.extern_html_root_url}"
        for e in args.extern
    )
    flags = [
        *rustdoc_invocation,
        "--out-dir",
        args.out_dir,
        *extern,
        *extern_html_root_urls,
    ]

    stdout = None if args.stdout_path is None else args.stdout_path.open("w")
    stderr = None if args.stderr_path is None else args.stderr_path.open("w")

    # remove output to ensure that we document into a clean directory
    shutil.rmtree(args.out_dir, ignore_errors=True)

    completed = run(flags, stdout=stdout, stderr=stderr)

    if args.fail:
        completed.check_returncode()

    if args.zip_to is not None:
        zip_dir(src=args.zip_from, dst=args.zip_to)

    if args.touch is not None:
        args.touch.touch()


def _get_ignore_parser() -> ArgumentParser:
    parser = ArgumentParser(
        description="ignore arguments in rustdoc invocation"
    )
    # ignored from //build/rbe/remote_action.py
    parser.add_argument("--remote-disable", nargs="*", help="ignored")
    parser.add_argument("--remote-inputs", nargs="*", help="ignored")
    parser.add_argument("--remote-outputs", nargs="*", help="ignored")
    parser.add_argument("--remote-output-dirs", nargs="*", help="ignored")
    parser.add_argument("--remote-flag", nargs="*", help="ignored")
    # ignored from //build/rust/rustc_link_attribute.gni
    parser.add_argument("-l", help="ignored")
    return parser


def _main_arg_parser() -> ArgumentParser:
    parser = ArgumentParser(
        description="runs rustdoc", fromfile_prefix_chars="@"
    )
    parser.add_argument(
        "--fail",
        default=True,
        action=BooleanOptionalAction,
        help="fail this wrapper if rustdoc fails",
    )
    parser.add_argument(
        "--stdout-path",
        type=Path,
        help="path to write rustdoc stdout",
    )
    parser.add_argument(
        "--stderr-path",
        type=Path,
        help="path to write rustdoc stderr",
    )
    parser.add_argument(
        "--touch",
        type=Path,
        help="Touch this file upon script completion",
    )
    parser.add_argument(
        "--extern",
        action="append",
        default=[],
        help="extern crates",
    )
    parser.add_argument(
        "--extern-html-root-url",
        type=str,
        required=True,
        help="use this as the --extern-html-root-url",
    )
    parser.add_argument(
        "--out-dir",
        required=True,
        type=Path,
        help="directory to place documentation",
    )
    parser.add_argument(
        "--aliased-deps-map",
        type=Path,
        help="rename aliases",
    )
    parser.add_argument(
        "--zip-from", type=Path, help="--out-dir of rustdoc invocation"
    )
    parser.add_argument(
        "--zip-to", type=Path, help="path to .zip file to append to or create"
    )
    parser.add_argument(
        "rustdoc_invocation",
        nargs="*",
        help="Provide full rustdoc invocation, including the rustdoc executable, and all intended flags. Will be forwarded to subprocess.run, along with generated externs",
    )

    parser.set_defaults(func=main)
    return parser


def _strip_cmd_prefixes(arg: str) -> str:
    for prefix in ("--remote-only=", "--local-only="):
        arg = arg.removeprefix(prefix)
    return arg


if __name__ == "__main__":
    parser = _main_arg_parser()
    args = parser.parse_args(argv[1:])
    ignore_parser = _get_ignore_parser()
    (_ignored, rustdoc_invocation) = ignore_parser.parse_known_args(
        args.rustdoc_invocation
    )
    # Strip prefixes to make sure flags from Rust configs are correctly applied
    # to rustdoc invocations. See https://fxbug.dev/407713438 for details.
    rustdoc_invocation = [
        _strip_cmd_prefixes(arg) for arg in rustdoc_invocation
    ]
    args.func(args, rustdoc_invocation)
