#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Verify the content of the test debug_symbols.json file and that all binaries it points to are unstripped."""

import argparse
import json
import sys
from pathlib import Path

# Allow this script to be run directly from the command-line to help debugging.
_SCRIPT_DIR = Path(__file__).parent
_FUCHSIA_DIR = _SCRIPT_DIR.parent.parent.parent.parent
_MODULES_DIR = _FUCHSIA_DIR / "build/python/modules"
sys.path.insert(0, str(_MODULES_DIR))

import elf


def check_debug_binary_path(path: Path) -> str:
    if not path.exists():
        return f"Missing debug binary file: {path}"

    info = elf.elfinfo.get_elf_info(str(path))
    if info is None:
        return f"Not an ELF file: {path}"

    if info.stripped:
        return f"Binary does not contain debug symbols: {path}"

    return ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest", type=Path, required=True, help="Input manifest path."
    )
    parser.add_argument(
        "--base-dir",
        type=Path,
        default=Path.cwd(),
        help="Base directory for file paths in manifest.",
    )
    args = parser.parse_args()

    with args.manifest.open("rt") as f:
        manifest = json.load(f)

    if not isinstance(manifest, list):
        print(f"ERROR: Manifest is not a JSON list!", file=sys.stderr)
        return 1

    if len(manifest) == 0:
        print(f"ERROR: Manifest is empty!", file=sys.stderr)
        return 1

    errors: list[str] = []

    for entry in manifest:
        debug_binary_path = args.base_dir / entry["debug"]
        error = check_debug_binary_path(debug_binary_path)
        if error:
            errors.append(error)

    if errors:
        print(f"ERROR: Found %d errors:" % len(errors), file=sys.stderr)
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
