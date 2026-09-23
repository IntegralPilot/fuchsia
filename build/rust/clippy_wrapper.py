#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Wrapper around clippy-driver to execute clippy checks for Rust targets."""

import argparse
import os
import subprocess
import sys
from pathlib import Path

# Add //build/python/modules to sys.path
_MODULES_DIR = Path(__file__).resolve().parent.parent / "python" / "modules"
sys.path.insert(0, str(_MODULES_DIR))

from depfile import DepFile


def transform_opt(opt: str) -> str | None:
    # Extract optarg from --opt=optarg
    if not opt:
        return None
    if opt.startswith("--local-only="):
        # Same transformation done in 'build/rbe/local-only.sh'
        return opt.split("=", 1)[1]
    if opt.startswith("--remote"):
        # pseudoflag for RBE parameters, drop it
        return None
    if opt.startswith("--error-format="):
        # This script will replace it with --error-format=json.
        return None
    return opt


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run clippy-driver on a Rust target."
    )
    parser.add_argument(
        "--output",
        required=True,
        type=Path,
        help="clippy file to output (required)",
    )
    parser.add_argument(
        "--jq", required=True, type=Path, help="path to 'jq' (required)"
    )
    parser.add_argument(
        "--deps",
        required=True,
        type=Path,
        help="path to .deps argfile (required)",
    )
    parser.add_argument(
        "--transdeps",
        required=True,
        type=Path,
        help="path to .transdeps argfile (required)",
    )
    parser.add_argument(
        "--depfile",
        type=Path,
        default=None,
        help="path to depfile to emit (optional)",
    )
    parser.add_argument(
        "--fail", action="store_true", help="clippy cause failure"
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="produce output without printing or failing",
    )
    parser.add_argument(
        "--clippy-only",
        action="store_true",
        help="filter only clippy-specific warnings/errors",
    )

    args, driver_args = parser.parse_known_args()

    # After -- the remaining args are the clippy-driver command and args set
    # in the clippy GN template.
    if not driver_args or driver_args.pop(0) != "--":
        parser.error("Expected '--' before clippy-driver command and arguments")

    # Filter and transform driver options (e.g. drop RBE pseudoflags,
    # strip --error-format to be replaced with json, and unwrap --local-only).
    filtered_driver_options = [
        transformed
        for opt in driver_args
        if (transformed := transform_opt(opt)) is not None
    ]

    # $deps_rspfile contains --externs for direct dependencies
    if args.transdeps.exists():
        transdeps = sorted(set(args.transdeps.read_text().split()))
    else:
        transdeps = []

    # Rewrite --externs to use rmetas where possible.
    # Use rmetas where they exist, to avoid requiring local copies of full rlibs.
    # Remote-build rlibs do not need to be downloaded, only their .rmeta are needed.
    # Assume $deps_rspfile is formatted with one --extern per line.
    deps_lines = args.deps.read_text().splitlines()
    alt_lines: list[str] = []
    for line in deps_lines:
        line_str = line.strip()
        if not line_str:
            continue
        if line_str.startswith("--extern=") and line_str.endswith(".rlib"):
            mapping = line_str[len("--extern=") :]
            if "=" in mapping:
                lib_name, lib_path = mapping.split("=", 1)
                rmeta_path = lib_path[: -len(".rlib")] + ".rmeta"
                # If the .rmeta exists, use it.
                if Path(rmeta_path).is_file():
                    alt_lines.append(f"--extern={lib_name}={rmeta_path}")
                else:
                    sys.stderr.write(
                        f"Expecting {rmeta_path} to exist, but did not find it.\n"
                    )
                    return 1
            else:
                alt_lines.append(line_str)
        else:
            # No change for all other cases, including --extern=...=*.so
            alt_lines.append(line_str)

    deps_alt_path = args.deps.with_name(args.deps.name + ".alt")
    deps_alt_path.write_text(("\n".join(alt_lines) + "\n") if alt_lines else "")

    command = [
        *filtered_driver_options,
        "-Zno_codegen",
        f"@{deps_alt_path}",
        *transdeps,
        f"--emit=metadata={args.output}.rmeta",
        "--error-format=json",
        "--json=diagnostic-rendered-ansi",
    ]

    if args.depfile:
        command.append(f"--emit=dep-info={args.depfile}")

    env = os.environ.copy()
    env["RUSTC_LOG"] = "error"

    with open(args.output, "w") as out_f:
        proc = subprocess.run(command, stderr=out_f, env=env)
    result = proc.returncode

    # clean-up temporary files
    if result == 0:
        if deps_alt_path.exists():
            deps_alt_path.unlink()

    if args.depfile and args.depfile.exists():
        # Relativize any absolute paths pointing into the build directory or source root.
        # Ninja requires depfile paths to match target outputs (relative to root_build_dir)
        # to ensure correct dependency tracking and avoid build graph non-convergence.
        # Reading the DepFile automatically normalizes and rebases all input/output
        # paths relative to the current working directory (root_build_dir).
        with open(args.depfile, "r") as f:
            dep_file = DepFile.read_from(f)

        # This writes out the depfile with the now-normalized paths.
        with open(args.depfile, "w") as f:
            dep_file.write_to(f)

    filter_expr = (
        '(.code.code//"" | startswith("clippy::")) and '
        if args.clippy_only
        else ""
    )

    # Print any detected lints if --quiet wasn't passed
    if not args.quiet:
        jq_cmd = [
            str(args.jq),
            "-sr",
            f'.[] | select({filter_expr}(.level == "error" or .level == "warning")) | .rendered',
            str(args.output),
        ]
        try:
            jq_proc = subprocess.run(jq_cmd)
            if jq_proc.returncode != 0:
                sys.stdout.write(args.output.read_text())
        except Exception:
            sys.stdout.write(args.output.read_text())

    # Only fail the build with a nonzero exit code if --fail was passed
    if args.fail:
        return result

    return 0


if __name__ == "__main__":
    sys.exit(main())
