#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import subprocess
import sys

"""
usage: fx update-rust-3p-outdated
Updates third_party/rust_crates/Cargo.toml based on the latest versions from
crates.io and configuration in //third_party/rust_crates/outdated.toml.

See https://fuchsia.dev/fuchsia-src/development/languages/rust/third_party.md
for more details.

Flags:

--no-build      Don't build update_crates or cargo-gnaw, use cached versions.
--no-vendor     Don't run `fx update-rustc-third-party` after updating crate versions.

For global options, try `fx help`.
"""


def error(str):
    """
    Mimic output styling from `fx-error $str`.
    """
    RED = "\033[91m"
    BOLD = "\033[1m"
    END = "\033[0m"
    print(RED + BOLD + "ERROR: " + END + str, file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(
        prog="fx update-rust-3p-outdated",
        description="Updates external Rust dependencies with latest from crates.io",
    )

    parser.add_argument(
        "--no-build",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Don't build update_crates or cargo-gnaw, use cached versions.",
    )
    parser.add_argument(
        "--no-vendor",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Don't run `fx update-rustc-third-party` after updating crate versions.",
    )
    parser.add_argument(
        "--fuchsia-dir",
        action="store",
        required=True,
        help="IGNORE: Inherited via `update-rust-3p-outdated` helper script.",
    )
    parser.add_argument(
        "--update-crates-bin",
        action="store",
        required=True,
        help="IGNORE: Inherited via `update-rust-3p-outdated` helper script.",
    )
    parser.add_argument(
        "--prebuilt-cmake-dir",
        action="store",
        required=True,
        help="IGNORE: Inherited via `update-rust-3p-outdated` helper script.",
    )
    parser.add_argument(
        "--prebuilt-rust-dir",
        action="store",
        required=True,
        help="IGNORE: Inherited via `update-rust-3p-outdated` helper script.",
    )
    parser.add_argument(
        "--prebuilt-rust-cargo-outdated-dir",
        action="store",
        required=True,
        help="IGNORE: Inherited via `update-rust-3p-outdated` helper script.",
    )
    args = parser.parse_args()

    UPDATE_CRATES_TARGET = "host-tools/update_crates"
    skip_build_arg = "--no-build" if args.no_build else ""
    os.environ["PATH"] += os.path.join(args.prebuilt_cmake_dir)

    if args.no_build and not os.path.exists(args.update_crates_bin):
        error(
            f"--no-build was specified, but `{args.update_crates_bin}` does not exist."
        )
        error("Rerun without --no-build to build update_crates.")
        return 1

    # Running fx build on host-tools/update_crates...
    if not args.no_build:
        try:
            subprocess.check_call(["fx", "build", UPDATE_CRATES_TARGET])
        except subprocess.CalledProcessError:
            error("Failed to build update_crates, see previous error message.")
            error("To retry an old build of update_crates, specify --no-build.")
            return 1

    # Crate updates
    print("Running update_crates...")
    try:
        subprocess.check_call(
            [
                args.update_crates_bin,
                "--manifest-path",
                f"{args.fuchsia_dir}/third_party/rust_crates/Cargo.toml",
                "--overrides",
                f"{args.fuchsia_dir}/third_party/rust_crates/outdated.toml",
                "update",
                "--cargo",
                f"{args.prebuilt_rust_dir}/bin/cargo",
                "--outdated-dir",
                args.prebuilt_rust_cargo_outdated_dir,
                "--config-path",
                f"{args.fuchsia_dir}/third_party/rust_crates/.cargo/config.toml",
            ],
            cwd=args.fuchsia_dir,
        )
    except subprocess.CalledProcessError:
        error("Failed to update crates.")
        return 1

    # Crate vendoring
    if not args.no_vendor:
        print("Running vendor script")
        try:
            subprocess.check_call(
                ["fx", "update-rustc-third-party", skip_build_arg]
            )
        except subprocess.CalledProcessError:
            error("Failed to run vendor script.")
            return 1

    # Check updates
    print("Running update_crates again to check...")
    try:
        subprocess.check_call(
            [
                args.update_crates_bin,
                "--manifest-path",
                f"{args.fuchsia_dir}/third_party/rust_crates/Cargo.toml",
                "--overrides",
                f"{args.fuchsia_dir}/third_party/rust_crates/outdated.toml",
                "check",
            ],
            cwd=args.fuchsia_dir,
        )
    except subprocess.CalledProcessError:
        error("Failed to check crates post update.")
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
