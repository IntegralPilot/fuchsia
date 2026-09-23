# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tool to check that the driver binary was built correctly."""

import argparse
import subprocess
import sys


def parse_args():
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--driver_binary",
        help="Path to the libdriver.so file",
        required=True,
    )
    parser.add_argument(
        "--readelf",
        help="Path to the readelf binary",
        required=True,
    )
    return parser.parse_args()


def run(*command):
    try:
        return subprocess.check_output(
            command,
            text=True,
        ).strip()
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        raise e


def _fail(msg):
    print(f"FAIL: {msg}")
    sys.exit(1)


def _assert_in(value, iterable, msg):
    if not value in iterable:
        _fail(msg)


def _assert_not_in(value, iterable, msg):
    if value in iterable:
        _fail(msg)


def get_exported_symbols(args):
    # contents will contain 2 header lines followed by the symbol table. We skip
    # the first first line and use the second to figure out how many columns we have.
    #
    # it will look something like
    #
    #  Symbol table '.dynsym' contains 129 entries:
    #    Num:    Value          Size Type    Bind   Vis       Ndx Name
    #      0: 0000000000000000     0 NOTYPE  LOCAL  DEFAULT   UND
    #      1: 0000000000000000     0 FUNC    GLOBAL DEFAULT   UND __zx_panic
    #
    contents = run(
        args.readelf, "-s", "--demangle", "-W", args.driver_binary
    ).splitlines()

    columns = contents[1].split()
    if columns[-1] != "Name":
        print("readelf returned an unexpected format.")
        sys.exit(1)

    symbols = []
    column_count = len(columns)
    for line in contents[2:]:
        line_values = line.split()
        if len(line_values) != column_count:
            # The first line usually doesn't have a name so we ignore it
            continue
        name = line_values[-1]
        symbols.append(name)

    return symbols


def verify_exported_symbols(symbols):
    # Initial validity check that our symbol table isn't huge which indicates something
    # went wrong. We choose 200 as an upper limit, we should figure out a better check
    # here. src/lib/driver_symbols/restricted_symbols.h contains a list of restricted
    # symbols but we do not have access to this list here. We should find a way to
    # sync up that list with something we can read here so we can verify that we are
    # not exporting something we shouldn't
    if len(symbols) > 200:
        _fail("Exporting too many symbols in the driver library.")

    # These symbols need to be exported
    expected_symbols = {
        "__fuchsia_driver_registration__",
    }
    if expected_symbols.isdisjoint(symbols):
        _fail(
            "Expected to find one of {} in exported symbols {}".format(
                expected_symbols, symbols
            )
        )


def verify_needed_libs(args):
    # Running readelf --needed-libs returns a response that looks like
    # NeededLibraries [
    #  libc.so
    #  libfdio.so
    #  libsvc.so
    #  libtrace-engine.so
    #  libzircon.so
    # ]
    # We strip off the first and last lines and then remove the whitepace from
    # each line.
    needed_libs = [
        l.strip()
        for l in run(
            args.readelf, "--needed-libs", args.driver_binary
        ).splitlines()[1:-1]
    ]

    expected_libs = [
        "libc.so",
        "libfdio.so",
        "libsvc.so",
        "libtrace-engine.so",
        "libzircon.so",
    ]

    for lib in expected_libs:
        _assert_in(
            lib, needed_libs, f"Failed to find {lib} in needed libraries"
        )

    blocked_libs = ["libc++.so.2", "libc++abi.so.1"]
    for lib in blocked_libs:
        _assert_not_in(
            lib, needed_libs, f"Unexpectedly found {lib} in needed libs."
        )


def main():
    args = parse_args()
    symbols = get_exported_symbols(args)

    verify_exported_symbols(symbols)
    verify_needed_libs(args)


if __name__ == "__main__":
    sys.exit(main())
