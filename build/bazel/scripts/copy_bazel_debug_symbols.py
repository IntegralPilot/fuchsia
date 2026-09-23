# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Copies debug symbols from bazel-out to the Ninja build directory's .build-id/."""

import argparse
import sys
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR.parent.parent / "api"))
from bazel_action_impl import copy_debug_symbols_to_build_dir
from debug_symbols import DebugSymbolsManifestParser


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Copy debug symbols from bazel-out to the Ninja build directory's .build-id/"
    )
    parser.add_argument("build_dir", type=str, help="The build directory.")
    args = parser.parse_args()
    build_dir = Path(args.build_dir).resolve()
    manifest_file = build_dir / "bazel_host_tests.debug_symbols.json"
    if not manifest_file.exists():
        print("Manifest file not found", file=sys.stderr)
        return 1

    parser = DebugSymbolsManifestParser(build_dir)
    parser.enable_build_id_resolution()
    try:
        parser.parse_manifest_file(manifest_file)
    except Exception as e:
        print(
            f"Error parsing debug symbols manifest file: {e}", file=sys.stderr
        )
        return 1

    if not parser.entries:
        return 0

    copy_debug_symbols_to_build_dir(build_dir, parser.entries)
    return 0


if __name__ == "__main__":
    sys.exit(main())
