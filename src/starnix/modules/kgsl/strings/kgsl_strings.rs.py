#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import re
import sys


def usage() -> None:
    print(
        "Usage:\n"
        "  strings.rs.py INPUT OUTPUT\n"
        "    INPUT    linux uapi rust file containing symbol definitions\n"
        "    OUTPUT   destination path for the magma header file to generate\n"
        "  Example: ./strings.rs.py ./arm64.rs ./strings.rs\n"
        "  Generates the magma header based on a provided json definition.\n"
        "  Description fields are generated in the Doxygen format."
    )


# License string for the top of the file.
def license() -> str:
    return (
        "// Copyright 2025 The Fuchsia Authors. All rights reserved.\n"
        "// Use of this source code is governed by a BSD-style license that can be\n"
        "// found in the LICENSE file.\n"
    )


# Defines necessary imports.
def imports() -> str:
    return "use starnix_uapi::uapi;\n"


# Gets a list of strings that match the provided symbol prefix.
def get_strings(lines: list[str], prefix: str) -> list[str]:
    pattern = re.compile(f"^.*({prefix}[^:]*).*$")
    return [match.group(1) for line in lines if (match := pattern.search(line))]


# Returns the rust code for a particular string matching function.
def make_string_func(lines: list[str], prefix: str) -> str:
    func = (
        f"#[rustfmt::skip]\n"
        f"pub fn {prefix.lower()}(value: u32) -> String {{\n"
        f"    #[allow(unreachable_patterns)]\n"
        f"    match value {{\n"
    )
    for symbol in get_strings(lines, prefix):
        func += f'        uapi::{symbol} | uapi::arch32::{symbol} => "{symbol}".to_string(),\n'
    func += (
        f'        _ => format!("Unknown {prefix} ({{value:#08x}})"),\n'
        f"    }}\n"
        f"}}\n"
    )
    return func


def main() -> int:
    PREFIXES = [
        "IOCTL_KGSL",
        "KGSL_PROP",
    ]
    if len(sys.argv) != 3:
        usage()
        return 2
    try:
        with open(sys.argv[1], "r") as file:
            with open(sys.argv[2], "w") as dest:
                lines = [
                    license(),
                    imports(),
                ]
                input_lines = file.read().splitlines()
                for prefix in PREFIXES:
                    lines.append(make_string_func(input_lines, prefix))
                dest.write("\n".join(lines))
                return 0
    except Exception as e:
        print(f"Error accessing files: {e}")
        usage()
        return 1


if __name__ == "__main__":
    sys.exit(main())
