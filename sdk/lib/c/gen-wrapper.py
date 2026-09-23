#!/usr/bin/env fuchsia-vendored-python

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import shlex
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        type=Path,
        metavar="FILE",
        required=True,
        help="Output file",
    )
    parser.add_argument(
        "prefix",
        nargs="+",
        help="Prefix wrapper script will apply to its arguments",
    )
    args = parser.parse_args()

    cmd = " ".join(
        [shlex.quote(word) for word in args.prefix] + ["""${1+"$@"}"""]
    )

    args.output.write_text(
        f"""#!/bin/sh
{cmd}
"""
    )

    args.output.chmod(0o755)


if __name__ == "__main__":
    main()
