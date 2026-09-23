#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import subprocess
import sys
import tempfile


def generate_depfile(outpath: str) -> str:
    # Just lie and say we depend on build.ninja so we get re-run every gen.
    # Despite the lie, this is more or less correct since we want to observe
    # every build graph change.
    #
    # This logic was adapted from generate_gn_desc.py.
    return "%s: build.ninja" % outpath


def write_if_changed(outpath: str, content: bytes) -> None:
    """
    Writes content to the file named outpath.

    If outpath already exists and contains content already, does nothing and
    does not bump the file modification time. This lets ninja skip downstream
    actions if we don't need to change anything.
    """
    try:
        with open(outpath, "rb") as f:
            existing_content = f.read()
        if content == existing_content:
            return
    except IOError:
        pass

    with open(outpath, "wb") as outfile:
        outfile.write(content)


def run_build_client(
    client_binary: str, fuchsia_dir: str, output: str, quiet: bool = False
) -> None:
    subprocess.run(
        (
            [
                sys.executable,
                client_binary,
                "--fuchsia-dir",
                fuchsia_dir,
            ]
            + (["--quiet"] if quiet else [])
            + [
                "target_metadata",
                "--output",
                output,
            ]
        ),
        check=True,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fuchsia-dir", required=True)
    parser.add_argument("--client-binary", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--depfile", required=True)
    parser.add_argument(
        "--quiet", required=False, default=False, action="store_true"
    )
    args = parser.parse_args()

    with tempfile.TemporaryDirectory() as temp_dir:
        temp_file = os.path.join(temp_dir, "target_metadata.json")
        run_build_client(
            args.client_binary, args.fuchsia_dir, temp_file, quiet=args.quiet
        )
        with open(temp_file, "rb") as f:
            partial_json = f.read()
            write_if_changed(args.output, partial_json)

    depfile = generate_depfile(args.output)
    with open(args.depfile, "w") as outfile:
        outfile.write(depfile)
    return 0


if __name__ == "__main__":
    sys.exit(main())
