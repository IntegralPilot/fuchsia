# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Creates an archive containing all artifacts required to run a Lacewing test.

The compressed directory structure is as follows:
```
dir/
├── <py_runtime>
├── <py_stdlibs>
├── <lacewing_test_pyz>
├── <shared library tree>
└── fidling
    └── gen
        └── ir_root
            ├── <fidl_lib_name>
            │   └── <fidl_lib_name>.fidl.json
            └── ...
```
"""

import argparse
import json
import os
import pathlib
import shutil
import sys
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument(
    "--cpython", help="path to the CPython runtime", required=True
)
parser.add_argument(
    "--cpython-stdlibs",
    help="path to the CPython standard libraries",
    required=True,
)
parser.add_argument(
    "--test-pyz",
    type=pathlib.Path,
    help="Path to the Laceweing test PYZ archive",
    required=True,
)
parser.add_argument(
    "--fidl-ir-list",
    type=pathlib.Path,
    help="Path to the file containing all FIDL IRs required by the test",
    required=True,
)
parser.add_argument(
    "--c-extension-library-tree",
    help="List of C extension shared libraries relative to build directory",
    nargs="*",
    required=True,
)
parser.add_argument(
    "--output",
    help="Path to output archive",
    required=True,
)
parser.add_argument(
    "--depfile",
    help="Path to the depfile to generate",
    type=argparse.FileType("w"),
    required=True,
)

# LINT.IfChange

# These artifacts are directly referenced by the self-extracting Rust binary's
# source code so we must use these exact file names in the archive.
PYTHON_RUNTIME_NAME: str = "python3"
LACEWING_PYZ_NAME: str = "test.pyz"

# LINT.ThenChange(//build/python/self_extracting_binary/src/main.rs)


# Exclude copying the standard library's `__pycache__` files as these caches may
# be modified after the output (Lacewing archive) is created which results in
# build failures such as:
#  `recorded mtime of <output> older than most recent input <__pycache__>`
PYCACHE_DIR: str = "__pycache__"


def _sanitize_paths(paths: list[str]) -> list[str]:
    """Sanitize file paths for use in GN depfile.

    Args:
        paths: The list of file paths to sanitize.

    Returns:
        A list of sanitized file paths.
    """
    # GN depfile require paths with whitespaces to be escaped.
    return [path.replace(" ", r"\ ") for path in paths]


def main() -> int:
    args = parser.parse_args()

    # List of undeclared input dependencies to the GN action.
    ins = []

    # Copy Lacewing artifacts into a temporary directory for compression.
    with tempfile.TemporaryDirectory() as td:
        # Python runtime.
        shutil.copy2(args.cpython, Path(td) / PYTHON_RUNTIME_NAME)

        # Lacewing test PYZ.
        shutil.copy2(args.test_pyz, Path(td) / LACEWING_PYZ_NAME)

        # Record `src` in `ins` for depfile before copying.
        def copy_and_record(src: str, dst: str) -> None:
            ins.append(src)
            shutil.copy2(src, dst)

        # Shared libraries.
        for c_extension in args.c_extension_library_tree:
            destination = Path(td) / os.path.dirname(c_extension)
            os.makedirs(destination, exist_ok=True)
            shutil.copy2(c_extension, destination)

        # Python standard libraries.
        shutil.copytree(
            args.cpython_stdlibs,
            Path(td) / os.path.basename(args.cpython_stdlibs),
            copy_function=copy_and_record,
            ignore=shutil.ignore_patterns(PYCACHE_DIR),
        )

        # FIDL IR.
        with args.fidl_ir_list.open() as f:
            fidl_ir_json = json.load(f)
            for entry in fidl_ir_json:
                lib_name = entry["library_name"]
                ir_source = entry["source"]
                ir_name = os.path.basename(ir_source)

                out_dir = Path(td) / "fidling" / "gen" / "ir_root" / lib_name
                out_dir.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ir_source, out_dir / ir_name)

                ins.append(ir_source)

        # Compress into archive.
        # TODO(https://fxbug.dev/346668324): Consider other formats to optimize
        # for size.
        shutil.make_archive(args.output.removesuffix(".zip"), "zip", td)

    # Generate depfile.
    ins = _sanitize_paths(ins)
    args.depfile.write("{}: {}\n".format(args.output, " ".join(ins)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
