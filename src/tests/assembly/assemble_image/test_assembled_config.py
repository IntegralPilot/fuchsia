# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import pathlib
import sys

from run_assembly import run_product_assembly


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run ffx assembly with the provided arguments."
    )
    parser.add_argument(
        "--ffx-bin",
        type=pathlib.Path,
        required=True,
        help="Path to ffx binary.",
    )
    parser.add_argument(
        "--product-assembly-config",
        type=pathlib.Path,
        required=True,
        help="Path to product assembly configuration input.",
    )
    parser.add_argument(
        "--board-config",
        type=pathlib.Path,
        required=True,
        help="Path to board configuration input.",
    )
    parser.add_argument(
        "--platform-artifacts",
        dest="platform_artifacts",
        type=pathlib.Path,
        required=True,
        help="Path to platform artifacts.",
    )
    parser.add_argument(
        "--legacy-bundle",
        type=pathlib.Path,
        required=False,
        help="Path to the legacy input bundle manifest.",
    )
    parser.add_argument(
        "--outdir",
        type=pathlib.Path,
        required=True,
        help="Path to output directory.",
    )
    parser.add_argument(
        "--gendir",
        type=pathlib.Path,
        required=True,
        help="Path to gendir directory.",
    )
    parser.add_argument(
        "--stamp",
        type=pathlib.Path,
        required=True,
        help="Path to stampfile for telling ninja we're done.",
    )
    parser.add_argument(
        "--config",
        action="append",
        required=False,
        help="Package config arguments.",
    )
    parser.add_argument(
        "--suppress-overrides-warning",
        action="store_true",
        required=False,
        help="Whether to display the developer overrides warning banner.",
    )
    parser.add_argument(
        "--developer-overrides",
        type=pathlib.Path,
        required=False,
        help="Developer overrides to add.",
    )
    parser.add_argument(
        "--mode",
        type=str,
        required=False,
        help="The assembly mode to use.",
    )
    args = parser.parse_args()

    kwargs = {}
    if args.config:
        kwargs["extra_config"] = args.config
    if args.developer_overrides:
        kwargs["developer_overrides"] = args.developer_overrides
    if args.mode:
        kwargs["mode"] = args.mode

    output = run_product_assembly(
        ffx_bin=args.ffx_bin,
        platform=args.platform_artifacts,
        product=args.product_assembly_config,
        board_config=args.board_config,
        platform_artifacts=args.platform_artifacts,
        legacy_bundle=args.legacy_bundle,
        outdir=args.outdir,
        gendir=args.gendir,
        suppress_overrides_warning=args.suppress_overrides_warning,
        **kwargs,
    )
    if output.returncode != 0:
        sys.exit(1)

    with open(args.stamp, "w") as f:
        pass  # creates the file

    return 0
