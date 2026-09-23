#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--rspfile",
        type=argparse.FileType("r"),
        help="File containing list of ELF file names",
        required=True,
    )
    parser.add_argument(
        "--libprefix-rspfile",
        type=argparse.FileType("r"),
        help="File containing libprefix for ELF file when installed",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        type=argparse.FileType("w"),
        required=True,
    )
    parser.add_argument(
        "--object",
        type=argparse.FileType("w"),
        help="Output file for object.inc fragment",
        required=True,
    )
    parser.add_argument(
        "--entry",
        type=argparse.FileType("w"),
        help="Output file for entry.inc fragment",
        required=True,
    )
    parser.add_argument("readelf", help="llvm-readelf binary", nargs=1)
    args = parser.parse_args()

    elf_files = [line[:-1] for line in args.rspfile.readlines()]
    args.rspfile.close()

    [libprefix] = [
        line[:-1] for line in args.libprefix_rspfile.readlines()
    ] or [""]
    args.libprefix_rspfile.close()

    args.depfile.write(f"{args.object.name}: {' '.join(elf_files)}\n")
    args.depfile.close()

    # Use Popen and json.load to stream the JSON output from llvm-readelf
    # directly into the parser. This avoids allocating a potentially massive
    # intermediate string for the entire JSON blob, which can cause memory
    # pressure or timeouts in large builds.
    readelf_cmd = [
        args.readelf[0],
        "--elf-output-style=JSON",
        "--program-headers",
        "--notes",
        "--dynamic",
    ] + elf_files
    with subprocess.Popen(readelf_cmd, stdout=subprocess.PIPE) as proc:
        data = json.load(proc.stdout)
        if proc.wait() != 0:
            raise subprocess.CalledProcessError(proc.returncode, readelf_cmd)

    def c_str(string):
        return '"' + string + '"'

    def process_file(file):
        source = file["FileSummary"]["File"]

        def write_preamble(output):
            output_dir = os.path.dirname(output.name)
            elf = os.path.relpath(source, output_dir)
            script = os.path.relpath(__file__, output_dir)
            output.write(
                f"// This file is generated from {elf} by {script}. DO NOT EDIT!\n"
            )

        def gen_notes():
            if "Notes" in file:
                # Old schema.
                for note in file["Notes"]:
                    yield note["NoteSection"]["Note"]
            else:
                # New schema.
                for notesection in file["NoteSections"]:
                    notesection = notesection["NoteSection"]
                    for note in notesection["Notes"]:
                        yield note

        def gen_load_segments():
            for phdr in file["ProgramHeaders"]:
                phdr = phdr["ProgramHeader"]
                if phdr["Type"]["Name"] != "PT_LOAD":
                    continue
                vaddr = phdr["VirtualAddress"]
                memsz = phdr["MemSize"]
                flags = phdr["Flags"]["Value"]
                yield f"kTestElfLoadSegment<{vaddr:#x}, {memsz:#x}, {flags:#x}>"

        build_id = None
        for note in gen_notes():
            build_id = note.get("Build ID")
            if build_id is not None:
                break
        assert build_id is not None, f"no Build ID in {file}"

        soname = [
            dyn["Name"]
            for dyn in file["DynamicSection"]
            if dyn["Type"] == "SONAME"
        ]
        if soname:
            [soname] = soname

        entry_name = f"kTestElfObject_{build_id}"
        write_preamble(args.entry)
        args.entry.write(f", {entry_name}\n")

        write_preamble(args.object)
        # The same object.inc file can be reached more than once in a TU by
        # dint of being included in multiple test_elf_load_set() targets being
        # rolled up into a single test_elf_source_set().
        args.object.write(f"#pragma once\n")
        # Also, there can be more than one test_elf_object() target for the
        # same actual file, e.g. evaluated in different toolchains.  (This is
        # ideally avoided, since it repeats the work of this script.)
        args.object.write(f"#ifndef DEFINED_{entry_name}\n")
        args.object.write(f"#define DEFINED_{entry_name} 1\n")
        args.object.write(
            f"inline constinit const TestElfObject {entry_name} = " + "{\n"
        )
        args.object.write(f"    .build_path = {c_str(source)},\n")
        args.object.write(f"    .build_id_hex = {c_str(build_id)},\n")
        if soname:
            args.object.write(f"    .soname = {c_str(soname)}_soname,\n")
        args.object.write(f"    .load_segments = kTestElfLoadSegments<\n")
        args.object.write(
            ",\n".join(
                [f"        {segment}" for segment in gen_load_segments()]
            )
        )
        args.object.write(">,\n")
        if libprefix:
            args.object.write(f"    .libprefix = {c_str(libprefix)},\n")
        args.object.write("};\n")
        args.object.write(f"#endif  // DEFINED_{entry_name}\n")

    for file in data:
        process_file(file)

    return 0


if __name__ == "__main__":
    sys.exit(main())
