# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Recursively copy all files from --source-dir to the dest-dir therein.
If a meta/manifest.json is found, the arch target will be merged into that file
(rather than replacing it).
"""

import argparse
import ast
import difflib
import filecmp
import json
import os
import re
import shutil
import subprocess

GENERATED_CONTANTS_TEMPLATE = """# AUTO-GENERATED - DO NOT EDIT!

# This file is re-generated in //scripts/sdk/bazel/merge_bazel_wrapper.py

# The following list of CPU names use Fuchsia conventions.
constants = struct(host_cpus = ["x64"], target_cpus = {target_cpu})
"""

WHITELIST_DIFF_LISTS = []
PATTERN = r"fuchsia_select\((.*?)\)"


def merge_fuchsia_select(source_file: str, dest_file: str) -> bool:
    if not os.path.basename(source_file).endswith(".bazel"):
        return False

    with open(source_file, "r", encoding="utf-8") as f:
        source = f.read()
        # If file does not contain fuchsia_select, then nothing to merge
        if "fuchsia_select" not in source:
            return False

    with open(dest_file, "r", encoding="utf-8") as f:
        dest = f.read()

    source_list = re.findall(PATTERN, source.replace("\n", ""))
    dest_list = re.findall(PATTERN, dest.replace("\n", ""))
    merged_map = {}
    for select_source, select_dest in zip(source_list, dest_list):
        dict_source = ast.literal_eval(select_source)
        dict_dest = ast.literal_eval(select_dest)
        dict_dest.update(dict_source)
        merged_dict_str = str(dict_dest).strip("{}")
        merged_map[select_dest.strip("{}").strip()] = merged_dict_str

    for src in merged_map:
        dest = dest.replace(src, merged_map[src])

    with open(dest_file, "w") as f:
        f.write(dest)

    return True


def merge_new_manifest(manifest_json, new_manifest):
    if manifest_json is None:
        manifest_json = new_manifest
    else:
        manifest_json["arch"]["target"].extend(new_manifest["arch"]["target"])

    return manifest_json


def print_file_difference(source_file, dest_file, relpath):
    print("=" * 40)
    print(relpath)
    print("=" * 40)
    with open(source_file) as sf, open(dest_file) as df:
        # Only print the difference if the files is utf-8 encoded
        try:
            for line in difflib.unified_diff(sf.readlines(), df.readlines()):
                print(line)
        except UnicodeDecodeError:
            return


def format_dir(buildifier_path: str, dir: str) -> None:
    cmd = [buildifier_path, "-r", dir]
    subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def sorting(item):
    if isinstance(item, dict):
        return sorted((key, sorting(values)) for key, values in item.items())
    if isinstance(item, list):
        return sorted(sorting(x) for x in item)
    return item


def validate_same_json(source_file, dest_file):
    if not os.path.basename(source_file).endswith(".json"):
        return False

    with open(source_file, "r+") as sf, open(dest_file, "r+") as df:
        source = sorting(json.load(sf))
        dest = sorting(json.load(df))

    return source == dest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-dir")
    parser.add_argument("--dest-dir")
    parser.add_argument("--buildifier-path", default=None)
    args = parser.parse_args()
    manifest_json = None

    if args.buildifier_path:
        for dir in [args.source_dir, args.dest_dir]:
            format_dir(args.buildifier_path, dir)

    for directory in os.listdir(args.source_dir):
        directory = os.path.join(args.source_dir, directory)
        for root, _, files in os.walk(directory):
            for f in files:
                source_file = os.path.join(root, f)
                relpath = os.path.relpath(source_file, directory)
                dest_file = os.path.join(args.dest_dir, relpath)
                if relpath == "meta/manifest.json":
                    with open(source_file, "r") as manifest_file:
                        new_manifest = json.load(manifest_file)
                    manifest_json = merge_new_manifest(
                        manifest_json, new_manifest
                    )
                    continue

                # Skip generated_constants.bzl file as we have to update the targets section with merged targets.
                if relpath == "generated_constants.bzl":
                    continue

                if os.path.exists(dest_file):
                    if filecmp.cmp(source_file, dest_file):
                        continue
                    if validate_same_json(source_file, dest_file):
                        continue
                    if not merge_fuchsia_select(source_file, dest_file):
                        print_file_difference(source_file, dest_file, relpath)
                    continue

                if not os.path.exists(os.path.dirname(dest_file)):
                    os.makedirs(os.path.dirname(dest_file))
                shutil.copy(source_file, dest_file)

    # Write meta/manifest.json
    dest_file = os.path.join(args.dest_dir, "meta/manifest.json")
    if not os.path.exists(os.path.dirname(dest_file)):
        os.makedirs(os.path.dirname(dest_file))
    with open(dest_file, "w") as f:
        json.dump(manifest_json, f, indent=2)

    # Write generated_constants.bzl
    dest_file = os.path.join(args.dest_dir, "generated_constants.bzl")
    with open(dest_file, "w") as f:
        f.write(
            GENERATED_CONTANTS_TEMPLATE.format(
                target_cpu=manifest_json["arch"]["target"]
            )
        )

    if args.buildifier_path:
        format_dir(args.buildifier_path, args.source_dir)


if __name__ == "__main__":
    main()
