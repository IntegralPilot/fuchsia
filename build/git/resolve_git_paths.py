#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import sys
from pathlib import Path

# Add current directory to sys.path to import git_utils
sys.path.insert(0, str(Path(__file__).resolve().parent))
from git_utils import get_git_path, get_git_ref


def main() -> int:
    source_root = Path(__file__).resolve().parents[2]

    projects = {
        "fuchsia": source_root,
        "mesa": source_root / "third_party/mesa-migrating/src",
        "glslang": source_root / "third_party/glslang/src",
        "spirv_tools": source_root / "third_party/spirv-tools/src",
    }

    results = {}
    for name, path in projects.items():
        if name in ("glslang", "spirv_tools") and not path.exists():
            continue
        head_file = get_git_path(path, "HEAD")
        ref_path = get_git_ref(path)

        results[f"{name}_head"] = str(head_file)
        results[f"{name}_ref"] = str(ref_path)

        if name == "fuchsia":
            index_file = get_git_path(path, "index")
            results[f"{name}_index"] = str(index_file)

    print(json.dumps(results))
    return 0


if __name__ == "__main__":
    sys.exit(main())
