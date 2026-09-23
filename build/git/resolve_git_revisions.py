#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import sys
from pathlib import Path

# Add current directory to sys.path to import git_utils
sys.path.insert(0, str(Path(__file__).resolve().parent))
from git_utils import get_git_revision


def main() -> int:
    source_root = Path(__file__).resolve().parents[2]
    projects = {
        "fuchsia": source_root,
        "mesa": source_root / "third_party/mesa-migrating/src",
    }

    results = {}
    # Fuchsia
    results["fuchsia_revision"] = get_git_revision(projects["fuchsia"])

    # Mesa
    mesa_rev = get_git_revision(projects["mesa"])
    results["mesa_revision"] = mesa_rev[:10]

    print(json.dumps(results))
    return 0


if __name__ == "__main__":
    sys.exit(main())
