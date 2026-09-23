#!/usr/bin/env fuchsia-vendored-python

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Life.  Don't talk to me about life.
"""

import argparse
import subprocess
import sys


def main() -> int:
    parser = argparse.ArgumentParser(argument_default=argparse.SUPPRESS)
    parser.add_argument(
        "--wrapper-run",
        metavar="CMD",
        action="append",
        default=[],
        help="Real program to invoke; repeat to add initial arguments",
    )
    parser.add_argument(
        "--wrapper-join",
        metavar="ARG",
        action="append",
        default=[],
        help="Exact single tokens to join with the following token",
    )
    parser.add_argument(
        "--wrapper-join-eq",
        metavar="ARG",
        action="append",
        default=[],
        help="Exact single tokens to join with '=' and the following token",
    )
    parser.add_argument(
        "--wrapper-elide",
        metavar="ARG",
        action="append",
        default=[],
        help="Exact single tokens to elide",
    )
    args, cmd_args = parser.parse_known_args()

    join_eq_set = set(args.wrapper_join_eq)
    join_set = set(args.wrapper_join) | join_eq_set
    elide_set = set(args.wrapper_elide)

    def gen_args():
        iter_args = iter(cmd_args)
        for arg in iter_args:
            if arg in join_set:
                sep = "=" if arg in join_eq_set else ""
                try:
                    arg += sep + next(iter_args)
                except StopIteration:
                    pass
            if arg not in elide_set:
                yield arg

    cmd = args.wrapper_run + [arg for arg in gen_args()]
    try:
        subprocess.run(cmd, check=True)
        return 0
    except FileNotFoundError:
        print(f"{cmd[0]:r} not found!\n", sys.stderr)
        result = 1
    except subprocess.CalledProcessError as e:
        result = e.returncode
    print(f"Exit {result} from {cmd}\n", sys.stderr)
    return result


if __name__ == "__main__":
    sys.exit(main())
