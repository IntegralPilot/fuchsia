#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Wrapper to run isort library, handling missing metadata.version
"""
import sys
from pathlib import Path
from importlib import metadata


def run_isort() -> None:
    """Runs isort, handling missing metadata"""
    isort_path = Path(__file__).parent / "src"
    sys.path.append(str(isort_path))

    # Handle missing version metadata in local isort source
    # This is a workaround for running isort directly from its source code,
    # which lacks the version metadata typically generated during a pip install.
    # TODO(b/353376647): Remove this workaround once https://github.com/PyCQA/isort/pull/2226
    # is merged and released in a new isort version.
    try:
        metadata.version("isort")
    except metadata.PackageNotFoundError:
        metadata.version = lambda x: "unknown"

    from src.isort.main import main
    main()

    sys.path.remove(str(isort_path))


if __name__ == "__main__":
    run_isort()
