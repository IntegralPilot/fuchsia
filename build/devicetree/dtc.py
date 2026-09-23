#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import argparse
import re
import subprocess
import sys

# (filename):(line number).(starting character)-(end character)[: ](error message)
# Warnings do not have a prefix, but Errors do. This is managed by the non-capturing
# group at the front.
TARGET_REGEX = r"(?:Error: )?(.*?):(\d+)\.(\d+)-\d+[: ]\s?(.*)$"
LINES_OF_CONTEXT = 5
LINENUM_WIDTH = 6  # 5 digits plus :
RED = "\x1b[0;31m"
YELLOW = "\x1b[0;33m"
RESET = "\x1b[0m"


def print_context(
    src_lines: list[str], error_line: int, start_char: int, errmsg: str
) -> None:
    """Prints the context and error/warning raised by dtc"""
    start_line = max(0, error_line - LINES_OF_CONTEXT)
    end_line = error_line + LINES_OF_CONTEXT

    for i, content in enumerate(
        src_lines, start=1
    ):  # enumerate starts at 0, lines start at 1
        if start_line <= i <= end_line:
            print(f"{i:5d}: {content.rstrip()}", file=sys.stderr)
            if i == error_line:
                print(
                    f"{' ' * (LINENUM_WIDTH + start_char)}{RED}^--- {errmsg}{RESET}",
                    file=sys.stderr,
                )


def print_dtc_stderr(dtc_stderr: str) -> None:
    """Annotates stderr for users"""
    has_warnings = False
    for line in dtc_stderr.strip().split("\n"):
        print(f"{YELLOW}{line}{RESET}", file=sys.stderr)
        if match := re.search(TARGET_REGEX, line):
            dts_file, line_num, start_char, errmsg = match.groups()

            src_lines = []
            try:
                with open(dts_file, "r", encoding="ascii") as f:
                    src_lines = f.readlines()
            except FileNotFoundError:
                print(f"Error: '{dts_file}' not found.", file=sys.stderr)
                return

            print_context(
                src_lines,
                int(line_num),
                int(start_char),
                errmsg,
            )
        elif "Warning" in line:
            has_warnings = True

    # Make it clear to the user that warnings are considered errors by our build.
    if has_warnings:
        print(
            f"{RED}Returning failure due to warnings in dts compilation{RESET}",
            file=sys.stderr,
        )

    # A newline separator between blocks
    print()


def dts_compile(args: argparse.Namespace, extra_dtc_args: list[str]) -> bool:
    """Compiles a dts to a dtbo and processes any output.
    Args:
        args: Necessary dtc args.
        extra_dtc_args: Additional dtc args that can be optionally provided in devicetree targets.

    Returns:
        True if there were any errors or warnings.
        False if successful.
    """
    # dtc [options] <input_file>
    cmd_args = [
        args.dtc_path,
        "--out-dependency=" + args.dep_file,
        "--out=" + args.dest_file,
    ]
    # An --include for each file listed in the includes_file
    cmd_args.extend(
        ["--include=" + line.strip() for line in args.includes_file]
    )
    cmd_args.extend(extra_dtc_args)
    cmd_args.append(args.src_file)

    command = subprocess.run(
        cmd_args, capture_output=True, text=True, check=False
    )

    if command.returncode or command.stderr:
        print("%s" % " \\ \r\n  ".join(command.args), file=sys.stderr)
        print_dtc_stderr(command.stderr)
        return True

    return False


def dtb_decompile(args: argparse.Namespace, extra_dtc_args: list[str]) -> bool:
    """Decompiles dtb to dts.
    Args:
        args: Necessary dtc args.
        extra_dtc_args: Additional dtc args that can be optionally provided in devicetree targets.

    Returns:
        True if there were any errors or warnings.
        False if successful.
    """
    cmd_args = [
        args.dtc_path,
        "--sort",
        "--in-format=dtb",
        "--out-format=dts",
        "--out=" + args.dest_file,
    ]
    cmd_args.extend(extra_dtc_args)
    cmd_args.append(args.src_file)

    command = subprocess.run(
        cmd_args, capture_output=True, text=True, check=False
    )
    if command.returncode or command.stderr:
        print("%s" % "\r\n\t".join(command.args), file=sys.stderr)
        print_dtc_stderr(command.stderr)
        return True

    return False


def main() -> int:
    """Handles argument parsing and then either compiles dts -> dtbo or decompiles dtb -> dts"""
    parser = argparse.ArgumentParser(prog="dtc.py")
    subparsers = parser.add_subparsers(dest="command")

    compile_parser = subparsers.add_parser(
        "compile", help="Compile a dts to dtb"
    )
    compile_parser.add_argument("dtc_path", help="devicetree compiler path")
    compile_parser.add_argument("src_file", help="dts path")
    compile_parser.add_argument("dest_file", help="dtb path")
    compile_parser.add_argument("dep_file", help="dependency file path")
    compile_parser.add_argument(
        "includes_file", type=argparse.FileType("r"), help="includes file path"
    )

    decompile_parser = subparsers.add_parser(
        "decompile", help="Decompile a dtb to dts"
    )
    decompile_parser.add_argument("dtc_path", help="devicetree compiler path")
    decompile_parser.add_argument("src_file", help="dtb path")
    decompile_parser.add_argument("dest_file", help="dts path")

    py_args, extra_dtc_args = parser.parse_known_args()
    if py_args.command == "decompile":
        return dtb_decompile(py_args, extra_dtc_args)
    return dts_compile(py_args, extra_dtc_args)


if __name__ == "__main__":
    sys.exit(main())
