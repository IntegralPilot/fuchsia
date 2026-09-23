#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Prints stat and diff of an upstream Chromium SHA across all maintained files."""

import argparse
import sys
import merge_helpers


def main():
    parser = argparse.ArgumentParser(
        description="Check upstream commit diff against all maintained Fuchsia files."
    )
    parser.add_argument("sha", help="Upstream Chromium SHA to inspect")
    parser.add_argument(
        "--stat-only", action="store_true", help="Print only --stat without full patch"
    )
    args = parser.parse_args()

    files = merge_helpers.MAINTAINED_FILES
    if not files:
        print("Error: MAINTAINED_FILES list is empty. Check maintained_files.txt.")
        sys.exit(1)

    cmd = [
        "git",
        "-C",
        merge_helpers.CHROMIUM_REPO_MEDIA,
        "show",
        "--stat" if args.stat_only else "-p",
        args.sha,
        "--",
    ] + files

    res = merge_helpers.run_cmd(cmd, check=False)
    if res.returncode != 0:
        print(f"Error checking SHA {args.sha}: {res.stderr}")
        sys.exit(res.returncode)

    out = res.stdout.strip()
    if not out:
        print(f"SHA {args.sha} does not touch any of the {len(files)} maintained files.")
    else:
        print(out)


if __name__ == "__main__":
    main()
