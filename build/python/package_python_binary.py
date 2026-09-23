#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Creates a Python zip archive for the input main source and
optionally type check for all the sources."""

import argparse
import json
import os
import shutil
import sys
import zipapp
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(
        "Creates a Python zip archive for the input main source"
    )
    parser.add_argument(
        "--target_name",
        help="Name of the build target",
        required=True,
    )
    parser.add_argument(
        "--main_source",
        help="Path to the source containing the main function",
        required=True,
    )
    parser.add_argument(
        "--main_callable",
        help="Name of the the main callable, that is the entry point of the generated archive",
        required=True,
    )
    parser.add_argument(
        "--gen_dir",
        help="Path to gen directory, used to stage temporary directories",
        required=True,
    )
    parser.add_argument("--output", help="Path to output", required=True)
    parser.add_argument(
        "--sources",
        help="Sources of this target, including main source",
        nargs="*",
    )
    parser.add_argument(
        "--data_package_name",
        help="Name of the Python package to store data sources in",
    )
    parser.add_argument(
        "--data_sources",
        help="Data sources of this target",
        nargs="*",
    )
    parser.add_argument(
        "--library_infos",
        help="Path to the library infos JSON file",
        type=argparse.FileType("r"),
        required=True,
    )
    parser.add_argument(
        "--depfile",
        help="Path to the depfile to generate",
        type=argparse.FileType("w"),
        required=True,
    )
    parser.add_argument(
        "--unbuffered",
        action="store_true",
        help="If set, pass argument to Python interpreter to not buffer output",
    )
    args = parser.parse_args()

    infos = json.load(args.library_infos)

    # Temporary directory to stage the source tree for this python binary,
    # including sources of itself and all the libraries it imports.
    #
    # It is possible to have multiple python_binaries in the same directory, so
    # using target name, which should be unique in the same directory, to
    # distinguish between them.
    app_dir = os.path.join(args.gen_dir, args.target_name)
    if os.path.exists(app_dir):
        # Temporary directory is likely left-over from prior incomplete builds.
        # Start from clean slate to prevent non-determinism.
        remove_dir(app_dir)
    os.makedirs(app_dir)

    # Copy over the sources of this binary.
    if (
        copy_binary_sources(
            dest_dir=app_dir,
            sources=args.sources,
            data_sources=args.data_sources,
            data_package_name=args.data_package_name,
        )
        != 0
    ):
        return 1

    # Make sub directories for all libraries and copy over their sources.
    lib_src_map: dict[str, str] = {}
    copy_library_sources(app_dir, infos, lib_src_map)
    args.depfile.write(
        "{}: {}\n".format(args.output, " ".join(lib_src_map.values()))
    )

    # Main module is the main source without its extension.
    main_module = os.path.splitext(os.path.basename(args.main_source))[0]
    # Manually create a __main__.py file for the archive, instead of using the
    # `main` parameter from `create_archive`. This way we can import everything
    # from the main module (create_archive only `import pkg`), which is
    # necessary for including all test cases for unit tests.
    #
    # TODO(https://fxbug.dev/42153163): figure out another way to support unit
    # tests when users need to provide their own custom __main__.py.
    main_file = os.path.join(app_dir, "__main__.py")
    with open(main_file, "w") as f:
        f.write(
            f"""
import sys
import inspect
from {main_module} import *

if inspect.iscoroutinefunction({args.main_callable}):
    import asyncio
    asyncio.run({args.main_callable}())
else:
    sys.exit({args.main_callable}())
"""
        )

    if args.unbuffered:
        interpreter = "/usr/bin/env -S fuchsia-vendored-python -u"
    else:
        interpreter = "/usr/bin/env fuchsia-vendored-python"

    try:
        zipapp.create_archive(
            app_dir,
            target=args.output,
            interpreter=interpreter,
            compressed=True,
        )

    finally:
        remove_dir(app_dir)
    return 0


def copy_binary_sources(
    dest_dir: str,
    sources: list[str],
    data_sources: list[str],
    data_package_name: str | None,
    src_map: dict[str, str] = {},
) -> int:
    """Copies the sources of this binary into the given destination directory.

    Args:
        dest_dir: The destination directory to copy the sources to.
        sources: The list of source file paths relative to the src_root.
        data_sources: The list of data source file paths.
        data_package_name: Name of the Python package to store data sources in.
        src_map: The mapping from the tmp dir to the original source paths.
    """
    for source in sources:
        basename = os.path.basename(source)
        if basename == "__main__.py":
            print(
                "__main__.py in sources of python_binary is not supported, see https://fxbug.dev/42153163",
                file=sys.stderr,
            )
            return 1
        dest = os.path.join(dest_dir, basename)
        src_map[dest] = source
        shutil.copy2(source, dest)

    if data_sources or data_package_name:
        if data_package_name is None:
            print(
                "--data_package_name must be provided if data sources exist.",
                file=sys.stderr,
            )
            return 1
        # Data sources are copied from their original locations to the
        # "data_package_name" directory relative to the PYZ root dir.
        dest_data_root = os.path.join(dest_dir, data_package_name)
        os.makedirs(dest_data_root, exist_ok=True)
        # Create __init__.py to make the data package importable.
        data_init_path = os.path.join(dest_data_root, "__init__.py")
        Path(data_init_path).touch()
        for data_src in data_sources:
            src = data_src
            dest = os.path.join(dest_data_root, os.path.basename(data_src))
            src_map[dest] = src
            shutil.copy2(src, dest)
    return 0


def _copy_files(
    files: list[str], src_root: str, dest_root: str, src_map: dict[str, str]
) -> None:
    """Helper to copy a list of files and maintain the source mapping."""
    for f in files:
        src = os.path.join(src_root, f)
        dest = os.path.join(dest_root, f)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        src_map[dest] = src
        shutil.copy2(src, dest)


def copy_library_sources(
    dest_dir: str,
    lib_infos: list[dict[str, object]],
    src_map: dict[str, str],
) -> None:
    """Copies the sources of all library dependencies of a binary target
        into the given destination directory.

    Args:
        dest_dir: The destination directory to copy the sources to.
        lib_infos: The list of library dependencies to be copied.
        src_map: The mapping from the temp dir source file paths to the original
    """
    for lib_info in lib_infos:
        dest_lib_root = os.path.join(dest_dir, str(lib_info["library_name"]))
        os.makedirs(dest_lib_root, exist_ok=True)

        _copy_files(
            lib_info["sources"],
            str(lib_info["source_root"]),
            dest_lib_root,
            src_map,
        )

        if lib_info.get("data_sources"):
            # Data sources are copied from their original location to the
            # "data_package_name" directory relative to the library_root.
            dest_data_root = os.path.join(
                dest_lib_root, lib_info["data_package_name"]
            )
            os.makedirs(dest_data_root, exist_ok=True)
            # Create __init__.py to make the data package importable.
            data_init_path = os.path.join(dest_data_root, "__init__.py")
            Path(data_init_path).touch()
            for data_src in lib_info["data_sources"]:
                src = data_src
                dest = os.path.join(dest_data_root, os.path.basename(data_src))
                src_map[dest] = src
                shutil.copy2(src, dest)

        if lib_info.get("stubs_root") and lib_info.get("stubs"):
            _copy_files(
                lib_info["stubs"],
                str(lib_info["stubs_root"]),
                dest_lib_root,
                src_map,
            )
    return


def copy_library_sources_for_mypy(
    dest_dir: str,
    lib_infos: list[dict[str, object]],
    src_map: dict[str, str],
) -> None:
    """Copies library components needed for mypy type checking.

    If a library has stubs associated with it but does not have mypy type checking,
    then only copy the stubs. This ensures mypy will only type check the stubs
    # when the library is included as a dependency.

    Args:
        dest_dir: The destination directory to copy the sources to.
        lib_infos: The list of library dependencies to be copied.
        src_map: The mapping from the temp dir source file paths to the original
    """
    for lib_info in lib_infos:
        dest_lib_root = os.path.join(dest_dir, str(lib_info["library_name"]))
        os.makedirs(dest_lib_root, exist_ok=True)

        if lib_info.get("mypy_enable", True):
            _copy_files(
                lib_info["sources"],
                str(lib_info["source_root"]),
                dest_lib_root,
                src_map,
            )

        if lib_info.get("stubs_root") and lib_info.get("stubs"):
            _copy_files(
                lib_info["stubs"],
                str(lib_info["stubs_root"]),
                dest_lib_root,
                src_map,
            )
    return


def remove_dir(root_dir_path: str) -> None:
    """
    Remove the given directory and all the files within it.

    Instead of using shutil.rmtree, this function manually walks the directory
    tree and removes files and directories one by one. This is necessary to
    avoid recording reads on directories, which can throw off the action tracer.

    Args:
        dir: The  directory to remove
    """
    for root, dirs, files in os.walk(root_dir_path, topdown=False):
        # Remove directories
        for dir in dirs:
            remove_dir(os.path.join(root, dir))
        # Remove files within this directory
        for file in files:
            file_path = os.path.join(root, file)
            os.remove(file_path)
    # Remove the top level directory
    os.rmdir(root_dir_path)
    return


if __name__ == "__main__":
    sys.exit(main())
