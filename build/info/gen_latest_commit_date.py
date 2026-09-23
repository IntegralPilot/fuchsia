#!/usr/bin/env fuchsia-vendored-python

# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script gets an estimate for the time of the most recent commit from the
# integration repo. This is used at runtime as a known-good lower bound on what
# the current time is for cases in which the clock hasn't yet been updated from
# the network.
#
# We do this by taking the CommitDate of the most recent commit from the
# integration repo. Unlike commits in other repositories, the commits in the
# integration repo are always created by server infrastructure, and thus their
# CommitDate fields are more reliable. This is critical since a backstop time
# which is erroneously in the future could break many applications.
#
# The commit date is then truncated to midnight UTC as the extra granularity
# is not needed for the kernel backstop time and because it causes extra work
# on infra builders.

import argparse
import os
import sys
from datetime import datetime, timezone

_SCRIPT_DIR = os.path.dirname(__file__)


def main():
    parser = argparse.ArgumentParser(
        description="Tool for extracting the latest commit date and hash from integration.git"
    )
    parser.add_argument(
        "--input-hash-file",
        required=True,
        help="Path to input commit hash file.",
    )
    parser.add_argument(
        "--input-stamp-file",
        required=True,
        help="Path to input commit stamp file.",
    )
    parser.add_argument(
        "--timestamp-file", help="path to write the unix-style timestamp to"
    )
    parser.add_argument(
        "--date-file", help="path to write the human-readable date string to"
    )
    parser.add_argument(
        "--commit-hash-file", help="path to write the commit hash itself to"
    )
    parser.add_argument(
        "--truncate",
        action="store_true",
        help="truncate the date to midnight UTC and use a fake commit hash",
    )

    args = parser.parse_args()

    with open(args.input_hash_file) as f:
        latest_commit_hash = f.read().strip()
        assert (
            latest_commit_hash != ""
        ), f"Input hash file is empty!: {args.input_hash_file}"

    with open(args.input_stamp_file) as f:
        latest_commit_date_unix = f.read().strip()
        assert (
            latest_commit_date_unix != ""
        ), f"Input stamp file is empty!: {args.input_stamp_file}"

    latest_commit_date = datetime.fromtimestamp(
        int(latest_commit_date_unix), timezone.utc
    )

    # Truncate the date if asked to do so, and replace the hash with one synthesized from the date
    if args.truncate:
        latest_commit_date = latest_commit_date.replace(
            hour=0, minute=0, second=0, microsecond=0
        )
        latest_commit_hash = f"{int(latest_commit_date.timestamp())}fedcba9876543210fedcba98765432"

    write_file_if_changed(
        args.timestamp_file, str(int(latest_commit_date.timestamp()))
    )
    write_file_if_changed(args.date_file, latest_commit_date.isoformat())
    write_file_if_changed(args.commit_hash_file, latest_commit_hash)


def write_file_if_changed(path: str, contents: str):
    if path:
        if contents_changed(path, contents):
            with open(path, "w") as file:
                file.write(contents)


def contents_changed(path: str, contents: str):
    try:
        with open(path, "r") as file:
            existing_contents = file.read()
            return existing_contents != contents
    except Exception:
        pass
    return True


if __name__ == "__main__":
    sys.exit(main())
