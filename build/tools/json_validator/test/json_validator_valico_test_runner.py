#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Wrapper script to run json_validator_valico against a test schema and document."""

import argparse
import os
import subprocess
import sys

import python.runfiles.runfiles as runfiles


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--expect-failure", action="store_true", help="The command should fail"
    )
    parser.add_argument(
        "json_validator_valico", help="Rlocation of validator tool"
    )
    parser.add_argument("test_schema", help="Rlocation of test schema")
    parser.add_argument("test_document", help="Rlocation of test document")
    parser.add_argument("extra_args", nargs=argparse.REMAINDER)

    args = parser.parse_args()

    r = runfiles.Create()

    def resolve(rlocation: str) -> str:
        path = r.Rlocation(rlocation)
        assert path, f"Impossible to locate file from Rlocation: {rlocation}"
        assert os.path.exists(
            path
        ), f"Resolved path does not exist for Rlocation: {rlocation}\n  --> {path}, PWD={os.getcwd()}, RUNFILES_DIR={os.environ.get('RUNFILES_DIR')}"
        return path

    json_validator_valico = resolve(args.json_validator_valico)
    test_schema = resolve(args.test_schema)
    test_document = resolve(args.test_document)

    command = [
        json_validator_valico,
        test_schema,
        test_document,
    ] + args.extra_args
    ret = subprocess.run(command)
    if args.expect_failure:
        if ret.returncode == 0:
            print(
                f"ERROR: Command succeeded unexpectedly: {command}",
                file=sys.stderr,
            )
            return 1
    else:
        if ret.returncode != 0:
            print(
                f"ERROR: Command failed unexpectedly: {command}",
                file=sys.stderr,
            )
            return 1

    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
