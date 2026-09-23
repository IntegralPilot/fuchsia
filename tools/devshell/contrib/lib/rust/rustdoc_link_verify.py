#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Checks generated docs for obvious failures.

We would like to avoid uploading broken documentation.

When this script exits unsuccessfully, do not upload the docs.

This script must strike a balance between ensuring that broken documentation is
not uploaded, and not being too picky. If we depend too much on specific rustdoc
implementation details, we could fail to upload correct docs.

"""

import sys
from argparse import ArgumentParser, RawTextHelpFormatter
from pathlib import Path
from sys import argv, stderr


def generator_empty(gen):
    is_empty = True
    for _ in gen:
        is_empty = False
        break
    return is_empty


def print_failure(*args):
    print(f"{Path(argv[0]).name}:", *args, file=stderr)


def main(doc_root: Path):
    all_okay = True

    def verify(condition, *args):
        if not condition:
            nonlocal all_okay
            all_okay = False
            print_failure(*args)

    # we can assume that our documentation comes with source code at this path, since it's commonly
    # linked to
    verify(
        (doc_root / "src").is_dir(),
        "expected src to be a directory in the docs",
    )
    # the 'host' path component is hardcoded in several places, so it's okay to assert it exists
    verify(
        (doc_root / "host").is_dir(),
        "expected host to be a directory in the docs",
    )
    verify(
        (doc_root / "host" / "src").is_dir(),
        "expected host/src to be a directory in the docs",
    )

    # check for certain types of files in crates with fuchsia targets
    fuchsia_root = doc_root
    verify(
        not generator_empty(fuchsia_root.rglob("index.html")),
        "expected fuchsia-side docs to have at least one index.html file",
    )
    verify(
        not generator_empty(fuchsia_root.rglob("*.css")),
        "expected fuchsia-side docs to have at least one css file",
    )
    verify(
        not generator_empty(fuchsia_root.rglob("*.js")),
        "expected fuchsia-side documentation to have at least one .js file",
    )
    verify(
        not generator_empty((fuchsia_root / "src").glob("*")),
        "expected at least one documented crate for fuchsia",
    )
    # check for certain types of files in crates with host targets
    host_root = doc_root / "host"
    verify(
        not generator_empty(host_root.rglob("index.html")),
        "expected host-side docs to have at least one index.html file",
    )
    verify(
        not generator_empty(host_root.rglob("*.css")),
        "expected host-side docs to have at least one css file",
    )
    verify(
        not generator_empty(host_root.rglob("*.js")),
        "expected host-side documentation to have at least one .js file",
    )
    verify(
        not generator_empty((host_root / "src").glob("*")),
        "expected at least one documented crate for the host",
    )

    if not all_okay:
        print_failure("failed verification")
        sys.exit(1)


def _main_arg_parser() -> ArgumentParser:
    # __doc__ refers to the doc comment at the top of this file. Use the raw formatter
    # so that newlines in the doc comment are newlines in the --help text.
    parser = ArgumentParser(
        description=__doc__,
        formatter_class=RawTextHelpFormatter,
    )
    parser.add_argument(
        "doc_root",
        type=Path,
        help="path to the root of generated documentation",
    )
    parser.set_defaults(func=main)
    return parser


if __name__ == "__main__":
    parser = _main_arg_parser()
    args = parser.parse_args(argv[1:])
    args.func(args.doc_root)
