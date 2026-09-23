#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import re
import subprocess
import sys

# Golden file build API module.
GOLDEN_FILES_JSON = "golden_files.json"

DESCRIPTION = """
Reruns golden file checks in the current build, relying on the
"golden_files" build API module. That is,  unless `--list` is passed, in
which case this command only prints out the associated goldens.

The goldens may be filtered by a substring of their source path (e.g., a
full source-relative directory) or by a regular expression; if no search
term is provided, all accessible golden files will be matched.

TODO(crbug.com/gn/301): This command may deal in stale metadata. Until this
bug is resolved, users must take care to make sure their build metadata is
up to data (e.g., via `fx gen`) before invoking this command.
"""


def main():
    parser = argparse.ArgumentParser(
        prog="fx check-goldens",
        formatter_class=argparse.RawTextHelpFormatter,
        description=DESCRIPTION,
    )
    parser.add_argument(
        "--list",
        help="Whether to just list the goldens instead of updating them",
        action="store_true",
    )
    parser.add_argument(
        "--regex",
        help="if set, the search term will be matched as a regular expression",
        action="store_true",
    )
    parser.add_argument(
        "--all",
        help="if set, includes otherwise ignored, dynamically-specified goldens (i.e., those specified via a build-time manifest)",
        action="store_true",
    )
    parser.add_argument(
        "search_term",
        help="search term to match golden files against",
        nargs="?",
    )
    args = parser.parse_args()

    build_dir = os.path.relpath(os.getenv("FUCHSIA_BUILD_DIR"))
    if build_dir is None:
        print("FUCHSIA_BUILD_DIR not set")
        return 1

    golden_files_json = os.path.join(build_dir, GOLDEN_FILES_JSON)
    if not os.path.exists(golden_files_json):
        subprocess.check_call(["fx", "gen"])

    with open(golden_files_json) as f:
        entries = json.load(f)

    # Account for comparisons given indirectly via build-time manifests,
    # ensuring that they are built before proceeding.
    if args.all:
        manifests = []
        for entry in entries:
            if "comparison_manifest" in entry:
                manifests.append(entry["comparison_manifest"])
        if manifests:
            subprocess.check_call(["fx", "build", "--"] + manifests)

    # Reload `entries`, as the JSON file may have been regenerated.
    with open(golden_files_json) as f:
        entries = json.load(f)

    def is_match(filename):
        if not args.search_term:
            return True
        if args.regex:
            return re.search(args.search_term, filename) != None
        return args.search_term in filename

    stamps = []
    matches = []
    for entry in entries:
        comparisons = []
        if "comparisons" in entry:
            comparisons = entry["comparisons"]
        elif args.all:
            manifest = os.path.join(build_dir, entry["comparison_manifest"])
            with open(manifest) as f:
                comparisons = json.load(f)

        matched = False
        for comparison in comparisons:
            golden = comparison["golden"]
            if is_match(golden):
                matched = True
                matches.append(golden)
        if matched:
            stamps.append(entry["stamp"])

    if args.list:
        print("\n".join(matches))
    elif stamps:
        subprocess.check_call(["fx", "build", "--"] + stamps)


if __name__ == "__main__":
    sys.exit(main())
