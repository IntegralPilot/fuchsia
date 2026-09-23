#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Wrapper script to run autoflake and import pyflakes.
"""
import sys
from pathlib import Path
from importlib import metadata


def run_autoflake() -> None:
    """Imports pyflakes and runs autoflake"""
    pyflakes_path = Path(__file__).parent.parent / "pyflakes" / "src"
    sys.path.append(str(pyflakes_path))

    autoflake_path = Path(__file__).parent / "src"
    sys.path.append(str(autoflake_path))

    from src.autoflake import main
    main()

    sys.path.remove(str(pyflakes_path))
    sys.path.remove(str(autoflake_path))

if __name__ == "__main__":
    run_autoflake()
