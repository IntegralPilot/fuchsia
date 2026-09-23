#!/usr/bin/env fuchsia-vendored-python

# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import fnmatch
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path
from typing import NoReturn


def eprint(*args, **kwargs) -> None:
    print(*args, file=sys.stderr, **kwargs)


def fail(*args, **kwargs) -> NoReturn:
    eprint("\033[31mError:\033[0m ", end="")
    eprint(*args, **kwargs)
    sys.exit(1)


def warn(*args, **kwargs) -> None:
    eprint("\033[33mWarning:\033[0m ", end="")
    eprint(*args, **kwargs)


def getenv(key: str) -> str:
    value = os.getenv(key)
    if value:
        return value
    fail(
        f"{key} environment variable not set. "
        "Are you running this command from an `fx` environment?",
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Dump disassembly for binaries to a file in the build directory.",
        epilog="""
Each BINARY can identify multiple binaries in one of the following forms:
  * ELF build ID, as a string of lowercase hex characters;
  * Unix filename pattern of the basename of a binary;
  * Unix filename pattern of the name of a GN target.

For example, among "libzircon.so" or "vmzircon", "*zircon*" will match both,
"*zircon" will only match "vmzircon", and "zircon" will match neither.

The disassembly will be written to files next to the binary file matches in
the build directory, using the matche's name with a ".lst" suffix.""",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--line-numbers",
        "-l",
        help="Display source line numbers (the default)",
        action="store_true",
        default=True,
    )
    parser.add_argument(
        "--no-line-numbers",
        help="Do not display source line numbers",
        dest="line_numbers",
        action="store_false",
    )
    parser.add_argument(
        "--source",
        "-S",
        help="Display source interleaved with the disassembly",
        action="store_true",
    )
    parser.add_argument(
        "--no-source",
        help="Do not display source interleaved with the disassembly (the default)",
        dest="source",
        action="store_false",
    )
    parser.add_argument(
        "--reloc",
        "-r",
        help="Display the relocation entries",
        action="store_true",
    )
    parser.add_argument(
        "--no-reloc",
        help="Do not display the relocation entries",
        dest="reloc",
        action="store_false",
    )
    parser.add_argument(
        "--demangle",
        help="Demangle symbol names (the default)",
        action="store_true",
        default=True,
    )
    parser.add_argument(
        "--no-demangle",
        "-n",
        help="Do not demangle symbol names",
        dest="demangle",
        action="store_false",
    )
    parser.add_argument(
        "--inlined-funcs",
        "-i",
        help="Display inlined function information (llvm-objdump only)",
        action="store_true",
        default=False,
    )
    parser.add_argument(
        "-G",
        "--gnu",
        action="store_true",
        help="Use GNU objdump rather than llvm-objdump.",
    )
    parser.add_argument(
        "-P",
        "--objdump-path",
        help="Use objdump binary at the specified path.",
    )
    parser.add_argument(
        "-e",
        "--edit",
        help="Automatically open the output disassembly file, provided $EDITOR is set",
        action="store_true",
    )
    parser.add_argument(
        "binaries",
        help="An ELF build ID or Unix file patterns for the name of a binary or GN target",
        nargs="*",
        metavar="BINARY",
    )
    args = parser.parse_args()

    if args.objdump_path:
        objdump_path = Path(args.objdump_path)
    elif args.gnu:
        prebuilt_binutils_dir = getenv("PREBUILT_BINUTILS_DIR")
        objdump_path = Path(prebuilt_binutils_dir) / "bin" / "objdump"
    else:
        prebuilt_clang_dir = getenv("PREBUILT_CLANG_DIR")
        objdump_path = Path(prebuilt_clang_dir) / "bin" / "llvm-objdump"

    if not objdump_path.exists():
        fail(f"no objdump not found at {objdump_path}")

    objdump_args = [objdump_path, "--disassemble"]
    if args.line_numbers:
        objdump_args.append("--line-numbers")
    if args.source:
        objdump_args.append("--source")
    if args.reloc:
        objdump_args.append("--reloc")
    if args.demangle:
        objdump_args.append("--demangle")

    # Enable inlined function display support when using llvm-objdump.
    if args.inlined_funcs and not args.gnu:
        objdump_args.append("--debug-inlined-funcs=limits-only")

    build_dir = Path(getenv("FUCHSIA_BUILD_DIR"))

    if len(args.binaries) == 0:
        fail("No binary names given!")
    all_matches = []
    for binary in args.binaries:
        matches = find_binaries(build_dir, binary)
        matches += [
            (path, f"local file {path}")
            for path in [Path(binary), build_dir / binary]
            if path.exists()
        ]
        all_matches += matches
        if len(matches) == 0:
            warn(f"'{binary}' did not match any binaries")
    if len(all_matches) == 0:
        fail("No matches")

    for bin_path, bin_label in all_matches:
        output_path = Path(f"{bin_path}.lst")
        print(f"Disassembling {bin_label}...")
        try:
            with output_path.open("w") as outfile:
                subprocess.run(
                    [*objdump_args, bin_path],
                    stdout=outfile,
                    stderr=subprocess.PIPE,
                    text=True,
                    check=True,
                )
        except FileNotFoundError:
            warn(
                f"Binary not found at '{bin_path}'; "
                f"run `fx build {bin_label}` to ensure that it is built",
            )
        except subprocess.CalledProcessError as e:
            fail(f"objdump failed for '{bin_path}': {e.stderr}")

        print(f"Wrote '{output_path}' for '{bin_label}'")

    if args.edit:
        # $EDITOR may contain other flags like `--wait` supplied in service of
        # `git commit` in one's terminal - but the base editor executable
        # followed by a file should always simply open the file for common
        # editors.
        editor = getenv("EDITOR").split()[0]
        if len(matches) > 1:
            warn("Multiple matches found; only opening the first in the editor")
        output_path = (build_dir / all_matches[0][0]).with_suffix(".lst")
        subprocess.run(
            f"{editor} {shlex.quote(str(output_path))}",
            stderr=subprocess.PIPE,
            check=True,
            shell=True,
            text=True,
        )


def find_binaries(
    build_dir: Path,
    bin_spec: str,
) -> list(tuple[str, str]):
    """
    Finds the binary (path, label) pairs in the build directory matching a binary spec.
    """
    binaries_json = build_dir / "binaries.json"
    if not binaries_json.exists():
        fail(
            f"'{binaries_json}' not found. Please run `fx gen`",
        )

    with binaries_json.open("r") as f:
        binaries = json.load(f)

    hex_re = re.compile(r"[0-9a-f]+")
    if re.fullmatch(hex_re, bin_spec):
        for binary in binaries:
            if "elf_build_id" not in binary:
                continue
            build_id_path = build_dir / binary["elf_build_id"]
            try:
                with build_id_path.open("r") as f:
                    build_id = f.read().strip()
            except FileNotFoundError:
                # It's possible the build ID file doesn't exist; skip it.
                continue
            if build_id == bin_spec:
                return [(binary["debug"], binary["label"])]
        fail(f"No binary with build ID '{bin_spec}' found")

    matches = []
    for binary in binaries:
        if "debug" not in binary:
            continue
        dist_name = Path(binary["dist"]).name
        debug_name = Path(binary["debug"]).name
        if (
            fnmatch.fnmatch(dist_name, bin_spec)
            or fnmatch.fnmatch(debug_name, bin_spec)
            or (
                "label" in binary
                and fnmatch.fnmatch(binary["label"], rf"*:{bin_spec}(*")
            )
        ):
            matches.append((build_dir / binary["debug"], binary["label"]))
    return matches


if __name__ == "__main__":
    main()
