#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Make the Emulator Support's Assembly Input Bundle

"""

import argparse
import json
import logging
import sys

from assembly import AIBCreator, AssemblyInputBundleCreationException

logger = logging.getLogger()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Create an assemblyinput bundle that includes the qemu kernel"
    )

    parser.add_argument(
        "--emulator-support-aib-input-metadata",
        type=argparse.FileType("r"),
        required=True,
    )
    parser.add_argument("--outdir", required=True)
    parser.add_argument(
        "--export-manifest",
        help="Path to write a FINI manifest of the contents of the AIB",
    )
    parser.add_argument(
        "--depfile",
        help="Path to write a depfile for the generated files adding the path read from the metadata file.",
    )
    args = parser.parse_args()

    emulator_support_aib_input_metadata = json.load(
        args.emulator_support_aib_input_metadata
    )

    # The build_api_module("images") entry with name "kernel" and type "zbi"
    # is the kernel ZBI to include in the bootable ZBI.  There can be only one.

    if len(emulator_support_aib_input_metadata) != 1:
        raise AssemblyInputBundleCreationException(
            "There must be exactly 1 `emulator_support_aib_input` in input metadata file. Path: "
            + args.emulator_support_aib_input_metadata.name
        )

    # The `zbi` entry in the `kernel_aib_input` metadata.
    aib_creator = AIBCreator(args.outdir)
    aib_creator.qemu_kernel = emulator_support_aib_input_metadata[0]["path"]

    (
        assembly_input_bundle,
        assembly_config_manifest_path,
        deps,
    ) = aib_creator.build()
    outfiles = [str(assembly_config_manifest_path)]

    # Write out a fini manifest of the files that have been copied, to create a
    # package or archive that contains all of the files in the bundle.
    if args.export_manifest:
        outfiles += [str(args.export_manifest.name)]
        with open(args.export_manifest, "w") as export_manifest:
            assembly_input_bundle.write_fini_manifest(
                export_manifest, base_dir=args.outdir
            )

    deplist = list(map(str, deps))
    with open(args.depfile, "w") as depfile:
        for outfile in outfiles:
            line = [outfile + ":"] + deplist
            depfile.write(" ".join(line) + "\n")


if __name__ == "__main__":
    try:
        main()
    except AssemblyInputBundleCreationException:
        logger.exception(
            "A problem occurred building the emulator support assembly input bundle"
        )
    finally:
        sys.exit()
