#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--ignore-unresolved-symbol",
        action="append",
        help="Expected undefined symbol name (may be repeated)",
        default=[],
    )
    parser.add_argument(
        "--ignore-all-unresolved-symbols",
        action="store_true",
        help="Allow all undefined symbol names",
        default=False,
    )
    parser.add_argument(
        "--init-array",
        action="store_true",
        help="Allow SHT_INIT_ARRAY sections",
        default=False,
    )
    parser.add_argument(
        "--fini-array",
        action="store_true",
        help="Allow SHT_FINI_ARRAY sections",
        default=False,
    )
    parser.add_argument(
        "--read-only-segments",
        action="store_true",
        help="Disallow PT_LOAD segments with PF_W",
        default=False,
    )
    parser.add_argument(
        "--depfile",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--stamp",
        type=Path,
        help="Stamp file written on success",
        required=True,
    )
    parser.add_argument(
        "--rspfile",
        type=Path,
        help="Response file listing name of ET_REL file to report",
        required=True,
    )
    parser.add_argument(
        "--objfile",
        type=Path,
        help="ET_REL file to examine",
    )
    parser.add_argument(
        "readelf",
        help="llvm-readelf binary",
        type=Path,
        nargs=1,
    )
    args = parser.parse_args()

    [file] = [line.strip() for line in args.rspfile.read_text().splitlines()]
    file = Path(file)

    objfile = args.objfile or file

    depfile_deps = " ".join(
        str(f) for f in [args.readelf[0], objfile, args.rspfile]
    )
    args.depfile.write_text(f"{args.stamp.name}: {depfile_deps}\n")

    readelf_cmd = [
        str(args.readelf[0]),
        "--elf-output-style=JSON",
        "--sections",
        "--symbols",
        "--program-headers",
        str(objfile),
    ]
    with subprocess.Popen(readelf_cmd, stdout=subprocess.PIPE) as proc:
        data = json.load(proc.stdout)
        if proc.wait() != 0:
            raise subprocess.CalledProcessError(proc.returncode, readelf_cmd)

    sections = [entry["Section"] for entry in data[0]["Sections"]]
    symbols = [entry["Symbol"] for entry in data[0]["Symbols"]]
    phdrs = [entry["ProgramHeader"] for entry in data[0]["ProgramHeaders"]]

    # The section with index 0 is always the null section header, but
    # llvm-readelf includes it just like any other.
    assert sections[0]["Name"]["Value"] == 0
    assert sections[0]["Type"]["Value"] == 0
    sections = sections[1:]

    init_sections = set(
        [
            section["Name"]["Name"]
            for section in sections
            if section["Type"]["Name"] == "SHT_INIT_ARRAY"
        ]
    )

    fini_sections = set(
        [
            section["Name"]["Name"]
            for section in sections
            if section["Type"]["Name"] == "SHT_FINI_ARRAY"
        ]
    )

    writable_load_segments = set(
        [
            phdr["VirtualAddress"]
            for phdr in phdrs
            if (
                phdr["Type"]["Name"] == "PT_LOAD"
                and any(
                    flag["Name"] == "PF_W" for flag in phdr["Flags"]["Flags"]
                )
            )
        ]
    )

    # The symbol with index 0 is always the null symbol, but llvm-readelf
    # includes it just like any other.
    assert symbols[0]["Name"]["Value"] == 0
    assert symbols[0]["Section"]["Value"] == 0
    symbols = symbols[1:]

    ok = True

    if not args.ignore_all_unresolved_symbols:
        allowed_symbols = set(args.ignore_unresolved_symbol)
        undefined_symbols = set(
            [
                symbol["Name"]["Name"]
                for symbol in symbols
                if symbol["Section"]["Value"] == 0
            ]
        )
        if not undefined_symbols <= allowed_symbols:
            ok = False
            sys.stderr.write(f"*** {file} FAILED undefined symbol check ***\n")
            for extra in sorted(undefined_symbols - allowed_symbols):
                sys.stderr.write(f"*** Unexpected reference: {extra}\n")
            sys.stderr.write(
                f"*** Find references with: llvm-objdump -drl {file}\n"
            )

    if init_sections and not args.init_array:
        ok = False
        sys.stderr.write(f"*** {file} FAILED .init_array check ***\n")
        for name in sorted(init_sections):
            sys.stderr.write(f"*** Unexpected SHT_INIT_ARRAY section {name}\n")

    if fini_sections and not args.fini_array:
        ok = False
        sys.stderr.write(f"*** {file} FAILED .fini_array check ***\n")
        for name in sorted(fini_sections):
            sys.stderr.write(f"*** Unexpected SHT_FINI_ARRAY section {name}\n")

    if writable_load_segments and args.read_only_segments:
        ok = False
        sys.stderr.write(f"*** {file} FAILED writable PT_LOAD check ***\n")
        for vaddr in sorted(writable_load_segments):
            sys.stderr.write(f"*** Unexpected writable PT_LOAD at {vaddr:#x}\n")

    if not ok:
        return 1

    args.stamp.write_text("OK\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
