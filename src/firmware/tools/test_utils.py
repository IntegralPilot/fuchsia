# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities for tests in src/firmware/tools."""

import pathlib
import pkgutil

_TEST_DATA_DIR = pathlib.Path(__file__).parent / "test_data"


def load_test_data(path: pathlib.Path) -> bytes:
    """Loads a test data file by path relative to test_data/.

    This supports both test data packaged into a `python_host_test()` as well as
    running the unittest file directly from the shell.
    """
    try:
        # python_host_test() flattens all sources into the archive root, so this module
        # is at the root and we look up files by basename rather than relative path.
        data = pkgutil.get_data(__name__, path.name)
        if data is not None:
            return data
    except OSError:
        pass

    local_path = _TEST_DATA_DIR / path
    if local_path.is_file():
        return local_path.read_bytes()

    raise FileNotFoundError(f"Failed to find test data file: {path}")
