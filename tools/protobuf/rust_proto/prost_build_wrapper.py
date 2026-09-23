#!/usr/bin/env fuchsia-vendored-python
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Wrapper around prost_build to generate a proper Ninja depfile."""

import argparse
import os
import re
import subprocess
import sys

# A regular expression for import() lines in protobuf input files.
_RE_IMPORT = re.compile(r'^\s*import(?:\s+public)?\s+"([^"]+)"\s*;\s*$')

# A regular expression for package lines in protobuf input files
_RE_PACKAGE = re.compile(r"^\s*package\s+([A-Za-z0-9_.]+)\s*;")


def ExtractImports(proto, proto_dir, import_dirs):
    """Compute all proto imports from an input .proto file.

    Args:
        proto: Input .proto file basename.
        proto_dir: Input .proto file current directory.
        import_dirs: List of include directories for imports.
    Returns:
        A set of file paths to all transitively imported files.
    """
    filename = os.path.join(proto_dir, proto)
    imports = set()

    with open(filename) as f:
        # Search the file for import.
        for line in f:
            match = _RE_IMPORT.match(line)
            if match:
                imported = match[1]

                # Check import directories to find the imported file.
                for candidate_dir in [proto_dir] + import_dirs:
                    candidate_path = os.path.join(candidate_dir, imported)
                    if os.path.exists(candidate_path):
                        imports.add(candidate_path)
                        # Transitively check imports.
                        imports.update(
                            ExtractImports(imported, candidate_dir, import_dirs)
                        )
                        break

    return imports


def ExtractPackages(protos):
    """Get the list of packages from a set of input .proto files.

    Args:
        protos: sequence or set of .proto file paths.
    Returns:
        A sorted list of package names as defined in the input files.
    """
    packages = set()
    for proto in protos:
        with open(proto) as f:
            for line in f:
                match = _RE_PACKAGE.match(line)
                if match:
                    package = match[1]
                    packages.add(package)
                    break

    return sorted(packages)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--protoc", required=True, help="Path to protoc compiler"
    )
    parser.add_argument(
        "--prost_build", required=True, help="Path to prost_build tool"
    )
    parser.add_argument("--out-dir", required=True, help="Output directory")
    parser.add_argument("--depfile", help="Ninja depfile output path.")
    parser.add_argument(
        "--expected-outputs",
        nargs="+",
        default=[],
        help="List of expected outputs",
    )
    parser.add_argument(
        "--expected-packages",
        nargs="+",
        default=[],
        help="List of expected packages",
    )
    parser.add_argument(
        "--gn-target", help="GN target label, used for error messages"
    )
    parser.add_argument(
        "--include-dirs",
        action="append",
        default=[],
        help="Import directory for protos.",
    )
    parser.add_argument(
        "--protos",
        action="append",
        default=[],
        help="Input protobuf definition file(s)",
    )

    args = parser.parse_args()

    cmd = [
        args.prost_build,
        "--protoc",
        args.protoc,
        "--out-dir",
        args.out_dir,
    ]
    for d in args.include_dirs:
        cmd += ["--include-dirs", d]

    for proto in args.protos:
        cmd += ["--protos", proto]

    subprocess.check_call(cmd)

    # Compute the list of transitive imports, i.e. implicit inputs.
    imports = set()
    for proto in args.protos:
        imports.update(
            ExtractImports(
                os.path.basename(proto),
                os.path.dirname(proto),
                args.include_dirs,
            )
        )

    # prost-build generates a lib.rs file, plus one <package>.rs file for
    # each package name used by the input protos or their transitive
    # imports.
    packages = ExtractPackages(args.protos + sorted(imports))
    if args.expected_packages:
        if not args.gn_target:
            parser.error("--expected-packages requires --gn-target value!")

        expected_packages = sorted(args.expected_packages)
        if expected_packages != packages:
            missing_packages = set(packages) - set(expected_packages)
            if missing_packages:
                error = f"ERROR: Missing `packages` values from {args.gn_target} definition:\n"
                for p in sorted(missing_packages):
                    error += f"  {p}\n"
                print(error, file=sys.stderr)
            extra_packages = set(expected_packages) - set(packages)
            if extra_packages:
                error = f"ERROR: Obsolete `packages` values from {args.gn_target} definition:\n"
                for p in sorted(extra_packages):
                    error += f"  {p}\n"
                print(error, file=sys.stderr)
            return 1

    outputs = sorted(
        [os.path.join(args.out_dir, "lib.rs")]
        + [os.path.join(args.out_dir, f"{package}.rs") for package in packages]
    )

    if args.expected_outputs:
        expected_outputs = sorted(args.expected_outputs)
        if expected_outputs != outputs:
            if not args.gn_target:
                parser.error("--expected-outputs requires --gn-target value!")
            missing_outputs = set(outputs) - set(expected_outputs)
            if missing_outputs:
                error = f"ERROR: Missing outputs from {args.gn_target} definition:\n"
                for out in sorted(missing_outputs):
                    error += f" {out}\n"
                error += "\nThis probably means an error in the implementation of rust_proto_library()!"
                print(error, file=sys.stderr)
                return 1
            extra_outputs = set(expected_outputs) - set(outputs)
            if extra_outputs:
                error = (
                    f"ERROR: Extra outputs from {args.gn_target} definition:\n"
                )
                for out in sorted(extra_outputs):
                    error += f" {out}\n"
                error += "\nThis probably means an error in the implementation of rust_proto_library()!"
                print(error, file=sys.stderr)
                return 1

    if args.depfile:

        def depfile_quote(path):
            return path.replace("\\", "\\\\").replace(" ", "\\ ")

        def to_depfile_list(args):
            return " ".join(depfile_quote(a) for a in args)

        with open(args.depfile, "w") as f:
            f.write(
                "%s: %s\n"
                % (to_depfile_list(outputs), to_depfile_list(sorted(imports)))
            )

    return 0


if __name__ == "__main__":
    sys.exit(main())
