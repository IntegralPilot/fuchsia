#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Creates a Python library for a C extension."""

import argparse
import os
import sys


def main():
    """Creates library wrapper"""

    parser = argparse.ArgumentParser(
        "Creates a Python library wrapper for a C extension."
    )
    parser.add_argument(
        "--target_name",
        help="Name of the build target",
        required=True,
    )
    parser.add_argument(
        "--shlib",
        help="Path to the shared library",
        required=True,
    )
    parser.add_argument(
        "--gen_dir",
        help="Path to gen directory, used to stage temp directories",
        required=True,
    )

    args = parser.parse_args()
    app_dir = os.path.join(args.gen_dir, args.target_name)
    os.makedirs(app_dir, exist_ok=True)
    main_file = os.path.join(app_dir, "__init__.py")

    shlib = os.path.basename(args.shlib)
    shlib_dir = os.path.dirname(args.shlib)
    # This file includes both `shlib` and the system search path `sys.path` so
    # that scripts not invoked in the root build dir can include the library.
    # Note: `{{search_path}}` below uses double curly braces for this generator f-string
    # so that it emits `{search_path}` in the generated script's f-string, where it is
    # evaluated at runtime.
    with open(main_file, "w", encoding="utf-8") as main_file_out:
        main_file_out.write(
            f"""
# DO NOT EDIT.
#
# This file was generated from //build/python/create_c_extension_import_hook.py
# which is invoked as part of the `python_c_extension` target defined in
# //build/python/python_c_extension.gni
from importlib.abc import Loader
import importlib.util
import importlib.machinery
import sys

def _init() -> object:
    # Scope import os locally to avoid namespace pollution for the import hook module.
    import os
    finder = importlib.machinery.PathFinder()
    curr_dir = os.path.abspath(os.path.dirname(__file__))
    search_path = [curr_dir, '{shlib_dir}'] + sys.path
    spec = finder.find_spec('{shlib}', path=search_path)
    if spec is None:
        raise ImportError(
            f"Couldn't find library '{shlib}.so' anywhere in path: {{search_path}}")
    mod = importlib.util.module_from_spec(spec)
    assert isinstance(spec.loader, Loader)
    spec.loader.exec_module(mod)
    return mod

globals().update(_init().__dict__)
"""
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
