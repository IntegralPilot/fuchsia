# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tool to check that the fuchsia package was built correctly."""

import argparse
import json
import struct
import subprocess
import sys
from collections.abc import Iterable
from typing import TypeVar

DEFAULT_CONFIGS = [
    "--no-environment",
    "--config",
    "daemon.autostart=false",
    "--config",
    "log.enabled=false",
]


def parse_args() -> argparse.Namespace:
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--meta_far",
        help="Path to the meta.far file",
        required=True,
    )
    parser.add_argument(
        "--ffx",
        help="Path to the ffx tool",
        required=True,
    )
    parser.add_argument(
        "--manifests",
        help="The expected component manifest paths (meta/foo.cm)",
        action="append",
        default=[],
        required=False,
    )
    parser.add_argument(
        "--structured_config_files",
        help="The expected structured config file paths (meta/foo.cvf)",
        action="append",
        default=[],
        required=False,
    )
    parser.add_argument(
        "--subpackages",
        help="The expected subpackage names",
        action="append",
        default=[],
        required=False,
    )
    parser.add_argument(
        "--bind_bytecode",
        help="The expected bind bytecode paths (meta/bind/foo.bindbc)",
        default=None,
        required=False,
    )
    parser.add_argument(
        "--package_name",
        help="Expected name of the package",
        required=True,
    )
    parser.add_argument(
        "--blobs",
        help="The exected blobs in dest=src format",
        action="append",
        default=[],
        required=False,
    )
    parser.add_argument(
        "--abi-revision",
        type=lambda x: int(x, base=16),
        help="Expected ABI revision, in hex",
        required=True,
    )
    return parser.parse_args()


def run(*command: str) -> str:
    try:
        return subprocess.check_output(
            command,
            text=True,
        ).strip()
    except subprocess.CalledProcessError as e:
        print(e.stdout)
        print(e.stderr)
        raise e


T = TypeVar("T")


def _assert_in(value: T, iterable: Iterable[T], msg: str) -> None:
    if not value in iterable:
        print("{}, {} not found in {}".format(msg, value, iterable))
        sys.exit(1)


def _assert_eq(a: T, b: T, msg: str) -> None:
    if a != b:
        print(Exception("{}: {} != {}".format(msg, a, b)))
        sys.exit(1)


def dest_merkle_pair_for_blobs(blobs: list[str], ffx: str) -> list[str]:
    if len(blobs) == 0:
        return []

    srcs = [blob.split("=")[1] for blob in blobs]
    merkles = [
        v.split(" ")[0].strip()
        for v in run(
            ffx,
            *DEFAULT_CONFIGS,
            "package",
            "file-hash",
            *srcs,
        ).split("\n")
    ]

    pairs = []
    for i, blob in enumerate(blobs):
        dest = blob.split("=")[0]
        pairs.append("{}={}".format(dest, merkles[i]))

    return pairs


def list_contents(args: argparse.Namespace) -> list[str]:
    """Lists the contents of the meta.far file"""
    return run(
        args.ffx,
        *DEFAULT_CONFIGS,
        "package",
        "archive",
        "list",
        args.meta_far,
    ).split("\n")


def filter_by_extension(inputs: list[str], extension: str) -> list[str]:
    extenstion_with_dot = ".{}".format(extension)
    return list(filter(lambda x: x.endswith(extenstion_with_dot), inputs))


def list_subpackage_names(args: argparse.Namespace) -> list[str]:
    return (
        json.loads(
            run(
                args.ffx,
                *DEFAULT_CONFIGS,
                "package",
                "archive",
                "cat",
                args.meta_far,
                "meta/fuchsia.pkg/subpackages",
            )
        )["subpackages"].keys()
        if "meta/fuchsia.pkg/subpackages" in list_contents(args)
        else []
    )


def check_contents_for_component_manifests(
    contents: list[str], manifests: list[str]
) -> None:
    for manifest in manifests:
        _assert_in(manifest, contents, "Failed to find component manifest")

    found_manifests = filter_by_extension(contents, "cm")
    _assert_eq(
        sorted(found_manifests),
        sorted(manifests),
        "The set of manifests failed to match",
    )


def check_contents_for_structured_config_files(
    contents: list[str], cvf_files: list[str]
) -> None:
    for cvf in cvf_files:
        _assert_in(cvf, contents, "Failed to find structured config file")

    found_files = filter_by_extension(contents, "cvf")
    _assert_eq(sorted(found_files), sorted(cvf_files), "cvf_files do not match")


def check_contents_for_subpackage_names(args: argparse.Namespace) -> None:
    actual = list_subpackage_names(args)
    expected = args.subpackages
    _assert_eq(
        sorted(actual),
        sorted(expected),
        "Expected subpackages not in package",
    )


def check_contents_for_bind_bytecode(
    contents: list[str], bind: str | None
) -> None:
    if bind:
        _assert_in(bind, contents, "Failed to find bind bytecode")


def check_for_abi_revision(args: argparse.Namespace) -> None:
    abi_stamp_contents = subprocess.check_output(
        [
            args.ffx,
            *DEFAULT_CONFIGS,
            "package",
            "archive",
            "cat",
            args.meta_far,
            "meta/fuchsia.abi/abi-revision",
        ]
    )

    # Parse little-endian uint64.
    abi_stamp = struct.unpack_from("<Q", abi_stamp_contents)[0]
    _assert_eq(abi_stamp, args.abi_revision, "ABI revision does not match")


def check_package_name(args: argparse.Namespace) -> None:
    contents = json.loads(
        run(
            args.ffx,
            *DEFAULT_CONFIGS,
            "package",
            "archive",
            "cat",
            args.meta_far,
            "meta/package",
        )
    )

    _assert_eq(
        contents["name"], args.package_name, "Package name does not match"
    )


def check_package_has_all_blobs(args: argparse.Namespace) -> None:
    dest_to_merkle = dest_merkle_pair_for_blobs(args.blobs, args.ffx)

    contents_str = run(
        args.ffx,
        *DEFAULT_CONFIGS,
        "package",
        "archive",
        "cat",
        args.meta_far,
        "meta/contents",
    )

    contents = contents_str.split("\n") if contents_str else []

    _assert_eq(
        sorted(contents),
        sorted(dest_to_merkle),
        "Expected blobs not in package contents",
    )


def main() -> None:
    args = parse_args()
    contents = list_contents(args)

    # TODO: add components as an arg
    check_contents_for_component_manifests(contents, args.manifests)
    check_contents_for_structured_config_files(
        contents, args.structured_config_files
    )
    check_contents_for_subpackage_names(args)
    check_contents_for_bind_bytecode(contents, args.bind_bytecode)
    check_for_abi_revision(args)
    check_package_name(args)
    check_package_has_all_blobs(args)


if __name__ == "__main__":
    main()
