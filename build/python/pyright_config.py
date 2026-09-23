#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Formats an input list of paths as extraPaths for pyrightconfig.json"""

import json
import os
import re
from pathlib import Path


def create_pyright_base_config(build_dir: Path, fuchsia_dir: Path) -> set[Path]:
    """Create a base config for pyright-based IDE integrations.

    Args:
        build_dir: Fuchsia build directory.
        fuchsia_dir: Fuchsia source directory.

    Returns:
        A set of Path values for the input files read by this function.
    """
    extra_python_paths_file = build_dir / "extra_python_dirs.json"
    source_paths_file = fuchsia_dir / "build/python/static_extra_paths.json5"
    output_file = build_dir / "pyrightconfig.base.json"
    build_directory_path = os.path.relpath(build_dir, fuchsia_dir)

    if not os.path.isfile(extra_python_paths_file):
        raise FileNotFoundError(
            f"Python paths file '{extra_python_paths_file}' does not exist."
        )

    if not os.path.isfile(source_paths_file):
        raise FileNotFoundError(
            f"Input file '{source_paths_file}' does not exist."
        )

    paths: set[str] = set()

    with open(extra_python_paths_file, "r") as input_file:
        input_data: object = json.load(input_file)
        if not isinstance(input_data, list):
            raise TypeError(
                f"Input file {extra_python_paths_file} must contain a list."
            )
        # FIDL paths are relative to the build directory, so we need to prepend that path.
        paths.update(
            [
                os.path.normpath(os.path.join(build_directory_path, path))
                for path in input_data
            ],
        )

    with open(source_paths_file, "r") as input_file:
        # Strip comments and trailing commas in containers. Pyright handles them fine,
        # but standard Python json does not like them.
        lines = [
            l for l in input_file.readlines() if not l.lstrip().startswith("//")
        ]
        content = re.sub(r",(\s*[}\]])", r"\1", "".join(lines))
        source_input: object = json.loads(content)
        if not isinstance(source_input, list):
            raise TypeError(
                f"Input file {source_paths_file} must contain a list."
            )
        paths.update(source_input)

    with open(output_file, "w") as out_file:
        output = {"extraPaths": sorted(paths)}
        json.dump(output, out_file, indent=2)

    return {source_paths_file}
