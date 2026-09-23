#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Compare the contents of an IDK to a golden file.

This script reads the manifest.json file from an exported IDK directory and
compares its contents to one or more existing golden files. If the contents of
the IDK don't match the contents of the golden file, this returns an error
indicating the set of changes that should be acknowledged by updating the golden
file.

Used by `sdk_final_manifest_golden` GN rules.
"""

import argparse
import dataclasses
import json
import os
import pathlib
import re
import sys
from collections.abc import Sequence
from typing import TextIO, TypedDict

HOST_TOOL_SCHEMES: Sequence[str] = [
    "host_tool",
    "ffx_tool",
    "companion_host_tool",
]

IdkPart = TypedDict("IdkPart", {"meta": str, "type": str})
IdkManifest = TypedDict("IdkManifest", {"parts": Sequence[IdkPart]})

# Looks like `#include "//foo/bar"`
INCLUDE_RE = re.compile(r'#include\s*"//([^"]+)"\s*')


@dataclasses.dataclass
class GoldenFile:
    path: pathlib.Path
    includes: list["GoldenFile"]
    atom_ids: set[str]

    @staticmethod
    def parse(path: pathlib.Path, source_root: pathlib.Path) -> "GoldenFile":
        includes = []
        atom_ids: set[str] = set()

        with path.open() as f:
            for line in f:
                if m := INCLUDE_RE.match(line):
                    include_path = source_root.joinpath(m[1])
                    includes += [GoldenFile.parse(include_path, source_root)]
                else:
                    atom_ids.add(line.strip())
        return GoldenFile(path=path, includes=includes, atom_ids=atom_ids)

    def print(self, source_root: pathlib.Path, file: TextIO) -> None:
        includes = [str(i.path.relative_to(source_root)) for i in self.includes]
        for include in sorted(includes):
            print('#include "//%s"' % include, file=file)
        for id in sorted(self.atom_ids):
            print(id, file=file)

    def inherited_atoms(self) -> set[str]:
        res = set()
        for include in self.includes:
            res |= include.all_atoms()
        return res

    def all_atoms(self) -> set[str]:
        return {*self.atom_ids, *self.inherited_atoms()}

    def all_manifest_paths(self) -> list[pathlib.Path]:
        res = [self.path]
        for include in self.includes:
            res += include.all_manifest_paths()
        return res


def part_to_id(part: IdkPart) -> str:
    """Get a string ID for a part of the IDK.

    Args:
    - part: An entry from the `parts` field of the IDK manifest.

    Returns:
      A string ID for that part, that's a bit more human-friendly than the
      JSON object.
    """
    path = part["meta"]
    meta_type = part["type"]
    if os.path.basename(path) == "meta.json":
        path = os.path.dirname(path)
    elif path.endswith("-meta.json"):
        path = path[: -len("-meta.json")]

    if not part.get("stable", False):
        path = f"{path} (unstable)"

    return f"{meta_type}://{path}"


def part_is_tool_for_other_cpu(part_id: str, host_cpu: str) -> bool:
    """Returns True if a given part is a host tool for a different cpu."""
    for scheme in HOST_TOOL_SCHEMES:
        if part_id.startswith(f"{scheme}://") and not part_id.startswith(
            f"{scheme}://tools/{host_cpu}/"
        ):
            return True
    return False


def remove_tools_for_other_cpus(part_ids: set[str], cpu: str) -> set[str]:
    """Takes a set of IDs and returns a set excluding host tools built
    for other CPU architectures."""
    return {id for id in part_ids if not part_is_tool_for_other_cpu(id, cpu)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=pathlib.Path,
        help="Path to the IDK manifest",
        required=True,
    )
    parser.add_argument(
        "--golden",
        type=pathlib.Path,
        help="Path to the golden file",
        required=True,
    )
    parser.add_argument(
        "--source_root",
        type=pathlib.Path,
        help="Path to the fuchsia source directory",
        required=True,
    )
    parser.add_argument(
        "--depfile",
        type=pathlib.Path,
        help="Path to output depfile",
    )
    parser.add_argument(
        "--only_verify_host_tools_for_cpu",
        help="If specified, ignore any host tools that aren't built for the named CPU arch.",
        required=False,
    )
    parser.add_argument(
        "--updated_golden",
        type=pathlib.Path,
        help=(
            "Path where a new version of the golden file will be written. The "
            "contents of this file will be meaningful iff "
            "--only_verify_host_tools_for_cpu is not specified"
        ),
        required=True,
    )
    parser.add_argument(
        "--label",
        help=(
            "GN label that caused this script to be run. Makes errors easier "
            "to reproduce."
        ),
    )

    args = parser.parse_args()

    with args.manifest.open() as f:
        manifest: IdkManifest = json.load(f)
    manifest_ids = {part_to_id(part) for part in manifest["parts"]}

    golden = GoldenFile.parse(args.golden, args.source_root)

    inherit_golden_ids = golden.inherited_atoms()
    effective_golden_ids = golden.all_atoms()

    added_ids: set[str] = manifest_ids - effective_golden_ids
    removed_ids: set[str] = effective_golden_ids - manifest_ids

    # Update the golden manifest file if possible.
    if args.only_verify_host_tools_for_cpu:
        added_ids = remove_tools_for_other_cpus(
            added_ids, args.only_verify_host_tools_for_cpu
        )
        removed_ids = remove_tools_for_other_cpus(
            removed_ids, args.only_verify_host_tools_for_cpu
        )

        # Write the file even if it's not useful as an updated golden, because
        # GN expects it to be there.
        with args.updated_golden.open("w") as f:
            print("Golden cannot be automatically updated.", file=f)
    else:
        with args.updated_golden.open("w") as f:
            golden.atom_ids = manifest_ids - inherit_golden_ids
            golden.print(args.source_root, f)

    if args.depfile:
        with args.depfile.open("w") as f:
            print(
                str(args.updated_golden),
                ": ",
                " ".join(str(p) for p in golden.all_manifest_paths()),
                file=f,
            )

    if added_ids:
        print("Parts added to IDK:", file=sys.stderr)
        for id in sorted(added_ids):
            print(" - " + id, file=sys.stderr)
    if removed_ids:
        print("Parts removed from IDK:", file=sys.stderr)
        for id in sorted(removed_ids):
            print(" - " + id, file=sys.stderr)
    if removed_ids or added_ids:
        print("Error: IDK contents have changed!", file=sys.stderr)
        if args.only_verify_host_tools_for_cpu:
            print(
                f"""\
The manifest cannot be automatically updated when not cross compiling host tools.

Please update the manifest manually - {args.golden}

or run fx set with "--args sdk_cross_compile_host_tools=true"
""",
                file=sys.stderr,
            )
        else:
            print(
                f"""\
Please acknowledge this change by running:

  cp {os.path.abspath(args.updated_golden)} {os.path.abspath(args.golden)}
""",
                file=sys.stderr,
            )

        print(
            f"""\
Note: If you are seeing this on an automated build failure and are trying to
reproduce, ensure that
    {args.label}
is in your GN graph.
""",
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
