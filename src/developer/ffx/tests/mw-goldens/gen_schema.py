#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generates schema for ffx commands"""

import argparse
import json
import os
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,  # Prepend helpdoc with this file's docstring.
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--command-list",
        type=str,
        required=True,
        help="Command list, one command per line to generate a schema for.",
    )
    parser.add_argument(
        "--schemalist",
        type=str,
        required=False,
        help=(
            "Destination file that will list all generated schema files."
            " If this argument is present, then the list is populated and "
            " no other args are processed"
        ),
    )
    parser.add_argument(
        "--goldens-dir",
        type=str,
        required=False,
        help="source directory for golden files.",
    )
    parser.add_argument(
        "--out-dir",
        type=str,
        required=False,
        help="output directory for schema information.",
    )
    parser.add_argument(
        "--ffx-path",
        type=str,
        required=False,
        help="path to ffx.",
    )
    parser.add_argument(
        "--tool-list",
        type=str,
        required=False,
        help="path to ffx subtool manifest file.",
    )
    parser.add_argument(
        "--comparisons",
        type=str,
        required=False,
        help="golden file test comparison file..",
    )
    parser.add_argument(
        "--depfile",
        type=str,
        required=False,
        help="output file containing the dependencies of the input. Used by GN.",
    )
    args = parser.parse_args()

    if args.schemalist:
        build_schemalist(args.command_list, args.schemalist)
    else:
        if not args.comparisons:
            raise ValueError("--comparisons option is missing")
        if not args.ffx_path:
            raise ValueError("--ffx-path option is missing")
        if not args.tool_list:
            raise ValueError("--tool-list option is missing")
        if not args.out_dir:
            raise ValueError("--outdir option is missing")
        if not args.goldens_dir:
            raise ValueError("--goldens-dir option is missing")
        if not args.depfile:
            raise ValueError("--depfile option is missing")

        build_command_list(
            args.command_list,
            args.out_dir,
            args.ffx_path,
            args.comparisons,
            args.goldens_dir,
            args.tool_list,
        )
        build_depfile(args.depfile, args.tool_list)


def build_schemalist(src_cmds, schemalist_path):
    with open(src_cmds) as input:
        with open(schemalist_path, mode="w") as output:
            for cmd in input.readlines():
                cmd = cmd.strip()
                cmd_parts = cmd.split(" ")
                schema_name = "_".join(cmd_parts[1:]) + ".json"
                print(schema_name, file=output)


def build_command_list(
    src_cmds, out_dir, ffx_path, comparison_path, goldens_dir, tool_list
):
    comparisons = []
    cmd_diagnostics = None

    with open(src_cmds) as input:
        for cmd in input.readlines():
            cmd = cmd.strip()
            cmd_parts = cmd.split(" ")
            schema_name = "_".join(cmd_parts[1:]) + ".json"
            schema_out_path = os.path.join(out_dir, schema_name)
            comparisons.append(
                {
                    "golden": os.path.join(goldens_dir, schema_name),
                    "candidate": schema_out_path,
                }
            )
            schema_cmd = [
                ffx_path,
                "--no-environment",
                "--config",
                f"ffx.subtool-manifest={tool_list}",
                "--schema",
                "--machine",
                "json-pretty",
            ] + cmd_parts[1:]
            with open(schema_out_path, "w") as schema_out:
                cmd_rc = subproce = subprocess.run(
                    schema_cmd, stdout=schema_out
                )
                if cmd_rc.returncode:
                    if not cmd_diagnostics:
                        cmds_cmd = [
                            ffx_path,
                            "--no-environment",
                            "--config",
                            f"ffx.subtool-manifest={tool_list}",
                            "commands",
                        ]
                        cmd_diagnostics = subprocess.check_output(cmds_cmd)
                    raise ValueError(
                        f"Error running {schema_cmd}: {cmd_rc.returncode} {cmd_rc.stdout} {cmd_rc.stderr}\nAll commands {cmd_diagnostics}"
                    )

    with open(comparison_path, mode="w") as cmp_file:
        json.dump(comparisons, cmp_file)


def build_depfile(depfile, tools_manifest):
    with open(tools_manifest, "r") as manifest:
        data = json.load(manifest)
    with open(depfile, "w") as f:
        for tool in data:
            print(
                f"{tools_manifest}: {tool['executable']} {tool['executable_metadata']}",
                file=f,
            )


if __name__ == "__main__":
    sys.exit(main())
