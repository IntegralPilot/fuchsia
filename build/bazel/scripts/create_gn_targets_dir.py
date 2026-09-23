#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generate a @gn_targets repository directory."""

import argparse
import sys
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))
import workspace_utils


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--build-dir",
        type=Path,
        required=True,
        help="Path to Ninja build directory.",
    )
    parser.add_argument(
        "--inputs-manifest",
        type=Path,
        required=True,
        help="Path to inputs manifest file.",
    )
    parser.add_argument(
        "--all_licenses_spdx_json",
        type=Path,
        required=True,
        help="Path tp SPDX file containing all license information related to the inputs.",
    )
    parser.add_argument(
        "--output-dir", type=Path, required=True, help="Output directory"
    )

    args = parser.parse_args()

    generated = workspace_utils.GeneratedWorkspaceFiles()

    workspace_utils.record_gn_targets_dir(
        generated,
        args.build_dir,
        args.inputs_manifest,
        args.all_licenses_spdx_json,
    )

    generated.write(args.output_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
