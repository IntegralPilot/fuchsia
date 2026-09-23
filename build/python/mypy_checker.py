#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""This script checks python type checking for the input sources."""
import argparse
import json
import os
import re
import subprocess
import sys
from enum import Enum
from pathlib import Path

import package_python_binary


class TargetType(str, Enum):
    LIBRARY = "library"
    BINARY = "binary"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target_type",
        type=TargetType,
        choices=[TargetType.LIBRARY, TargetType.BINARY],
        default=TargetType.LIBRARY,
        help="Whether the target is a binary or a library.",
    )
    parser.add_argument(
        "--target_name",
        help="Name of the build target",
        required=True,
    )
    parser.add_argument(
        "--library_infos",
        help="Path to the library infos JSON file",
        type=argparse.FileType("r"),
        required=True,
    )
    parser.add_argument(
        "--sources",
        help="The list of python source files for binaries",
        nargs="+",
    )
    parser.add_argument(
        "--data_sources",
        help="The list of data source file paths for binaries.",
        nargs="*",
    )
    parser.add_argument(
        "--data_package_name",
        help="Name of the Python package to store data sources in.",
    )
    parser.add_argument(
        "--gen_dir",
        help="Path to gen directory, used to stage temporary directories",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--depfile",
        help="Path to the depfile to generate",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--output",
        help="Path to output file",
        required=True,
    )
    parser.add_argument(
        "--cache_dir",
        help="Path to the cache directory for mypy",
        type=Path,
        required=True,
    )

    args = parser.parse_args()

    if args.target_type == TargetType.LIBRARY:
        # These options only apply to binaries, as libraries get all their
        # needed info from the library_infos
        if args.sources:
            parser.error(
                "'--sources' can only be used with '--target_type binary'"
            )
        if args.data_sources:
            parser.error(
                "'--data_sources' can only be used with '--target_type binary'"
            )
        if args.data_package_name:
            parser.error(
                "'--data_package_name' can only be used with '--target_type binary'"
            )

    lib_infos = json.load(args.library_infos)

    match args.target_type:
        case TargetType.LIBRARY:
            retval = run_mypy_on_library_target(
                args.target_name,
                args.gen_dir,
                lib_infos,
                args.output,
                args.depfile,
                args.cache_dir,
            )
        case TargetType.BINARY:
            retval = run_mypy_on_binary_target(
                args.target_name,
                args.gen_dir,
                args.sources,
                args.data_sources,
                args.data_package_name,
                lib_infos,
                args.output,
                args.depfile,
                args.cache_dir,
            )
        case default:
            # Here so that type checking knows that all code paths either
            # set 'retval' or raise an error.
            parser.error(f"unknown target type: {args.target_type}")

    if retval == 0:
        # To satisfy GN action requirements, creating a necessary empty output file on success.
        Path(args.output).touch()

    return retval


def run_mypy_on_binary_target(
    target_name: str,
    gen_dir: Path,
    src_files: list[str],
    data_sources: list[str],
    data_package_name: str | None,
    lib_infos: list[dict[str, object]],
    output_file: Path,
    depfile: Path,
    cache_dir: Path,
) -> int:
    """
    Runs `mypy` type checking on the sources of a binary target, as well as the
    sources of all its library dependencies that have "mypy_enable = True".

    Args:
        target_name: The name of the target being built.
        gen_dir: The path to the generated directory
        src_files: The list of sources of the binary target.
        data_sources: The list of data source file paths.
        data_package_name: Name of the Python package to store data sources in.
        lib_infos: The list of library dependencies of the binary target
        output_file: The path to the output stamp file
        depfile: The path to the depfile to generate
        cache_dir: The path to the incremental cache directory

    Returns:
        The exit code of the Mypy invocation.
    """
    # Temporary directory to stage the source tree for Mypy checks,
    # including sources of itself and all the libraries it imports.
    tmp_dir: str = os.path.join(gen_dir, target_name + "_bin_mypy")
    try:
        if os.path.exists(tmp_dir):
            package_python_binary.remove_dir(tmp_dir)
        os.makedirs(tmp_dir)

        # Mapping from the temp dir source file paths to the original source file paths
        src_map: dict[str, str] = {}

        # Copy over the sources of this target to the tmp directory.
        ret = package_python_binary.copy_binary_sources(
            dest_dir=tmp_dir,
            sources=src_files,
            data_sources=data_sources,
            data_package_name=data_package_name,
            src_map=src_map,
        )
        if ret != 0:
            return ret

        # Copy mypy enabled library sources to the tmp directory.
        package_python_binary.copy_library_sources_for_mypy(
            tmp_dir,
            [info for info in lib_infos if info["mypy_support"]],
            src_map,
        )

        # Write the depfile
        depfile.write_text(
            "{}: {}\n".format(output_file, " ".join(list(src_map.values())))
        )

        ret = run_mypy_checks(tmp_dir, src_map, cache_dir)
    finally:
        package_python_binary.remove_dir(tmp_dir)
    return ret


def run_mypy_on_library_target(
    target_name: str,
    gen_dir: Path,
    lib_infos: list[dict[str, object]],
    output_file: Path,
    depfile: Path,
    cache_dir: Path,
) -> int:
    """
    Runs `mypy` type checking on the sources of a library target, as well as the
    sources of all its library dependencies that have "mypy_enable = True".

    Args:
        target_name: The name of the target being built.
        gen_dir: The path to the generated directory
        lib_infos: The list of library dependencies of the library target
        output_file: The path to the output stamp file
        depfile: The path to the depfile to generate
        cache_dir: The path to the incremental cache directory

    Returns:
        The exit code of the Mypy invocation.
    """
    # Temporary directory to stage the source tree for Mypy checks,
    # including sources of itself and all the libraries it imports.
    tmp_dir = gen_dir / f"{target_name}_lib_mypy"
    if tmp_dir.exists():
        package_python_binary.remove_dir(str(tmp_dir))
    os.makedirs(tmp_dir)

    # Mapping from the temp dir source file paths to the original source file paths
    src_map: dict[str, str] = {}

    # Copy mypy enabled library sources to the tmp directory.
    package_python_binary.copy_library_sources_for_mypy(
        str(tmp_dir),
        [info for info in lib_infos if info["mypy_support"]],
        src_map,
    )

    # Write the depfile
    depfile.write_text(
        "{}: {}\n".format(output_file, " ".join(list(src_map.values())))
    )

    try:
        ret = run_mypy_checks(str(tmp_dir), src_map, cache_dir)
    finally:
        package_python_binary.remove_dir(str(tmp_dir))
    return ret


def run_mypy_checks(
    app_dir: str,
    src_map: dict[str, str],
    cache_dir: Path,
    with_incremental_cache: bool = True,
) -> int:
    """
    Runs `mypy` type checking on the app_dir, refactors the output to reflect
    the original file paths.

    Args:
        app_dir: The path of the directory to run type checking on
        src_map: Mapping from the original source file paths to app dir paths
        cache_dir: Path to the incremental cache directory.
        with_incremental_cache: If True, use the incremental cache.
            This is known to cause issues in infra due to
            https://fxbug.dev/345717802, so if this option is set
            the operation will be re-run with the cache disabled.

    Returns:
        int: returncode if type checking was successful, else error returncode
    """
    fuchsia_dir = Path(__file__).parent.parent.parent
    config_path = fuchsia_dir / "pyproject.toml"
    pylibs_dir = fuchsia_dir / "third_party" / "pylibs"
    try:
        return subprocess.run(
            (
                [
                    sys.executable,
                    "-S",
                    "-m",
                    "mypy",
                ]
                + (
                    [
                        # TODO(https://fxbug.dev/345717802): Disabling cache to temporarily resolve this flake.
                        # see https://github.com/python/mypy/issues/7276 for details on similar issue.
                        "--no-incremental",
                    ]
                    if not with_incremental_cache
                    else (
                        [
                            "--cache-dir",
                            str(cache_dir),
                        ]
                    )
                )
                + [
                    "--config-file",
                    str(config_path),
                    app_dir,
                ]
            ),
            env={
                "PYTHONPATH": os.pathsep.join(
                    [
                        str(pylibs_dir / "mypy" / "src"),
                        str(pylibs_dir / "mypy_extensions" / "src"),
                        str(pylibs_dir / "typing_extensions" / "src" / "src"),
                    ]
                )
            },
            capture_output=True,
            text=True,
            check=True,
        ).returncode
    except subprocess.CalledProcessError as e:
        if e.returncode != 0:
            if with_incremental_cache:
                # Try again without the incremental cache.
                return run_mypy_checks(
                    app_dir,
                    src_map,
                    cache_dir=cache_dir,
                    with_incremental_cache=False,
                )
            if e.stdout:
                refactored_out = convert_mypy_output(e.stdout, src_map)
                print(
                    f"\nPlease fix the following Mypy errors:\n{refactored_out}\n",
                    file=sys.stderr,
                )
            else:
                print(
                    f"\nError occured during Mypy type checking:\n{e.stderr}\n",
                    file=sys.stderr,
                )
        return e.returncode


def convert_mypy_output(output: str, src_map: dict[str, str]) -> str:
    """
    Converts the mypy output to use the source file paths as specified in
    src_map instead of the temp directory paths.

    Args:
        output: The mypy output to refactor
        src_map: A dictionary mapping from the original source file paths to the
            corresponding temp directory source file paths

    Returns:
        A refactored version of mypy output with the original source file paths
    """
    output_lines = output.splitlines()

    # e.g. "src/tests/end_to_end/rtc/test_rtc_conformance.py:12: error:"
    source_matcher = re.compile(r"^(\S+\.py):\d+")

    for i, line in enumerate(output_lines):
        match = source_matcher.search(line)
        if match:
            source = match.group(1)
            output_lines[i] = line.replace(
                source, src_map[source].replace("../../", "")
            )

    return "\n".join(output_lines)


if __name__ == "__main__":
    sys.exit(main())
