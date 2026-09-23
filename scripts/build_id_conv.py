#!/usr/bin/env fuchsia-vendored-python
# Copyright 2018 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
This script allows for the conversion between ids.txt and .build-id formats.
"""

import argparse
import errno
import os
from typing import Callable


def abs_path(path: str, rel_to: str) -> str:
    assert os.path.isabs(rel_to)
    if os.path.isabs(path):
        return path
    else:
        return os.path.abspath(os.path.join(rel_to, path))


def read_ids_txt(ids_path: str, rel_to: str) -> dict[str, str]:
    assert os.path.isabs(rel_to)
    with open(ids_path) as f:
        return {
            build_id: abs_path(path, rel_to)
            for (build_id, path) in (x.split() for x in f.readlines())
        }


def read_build_id_dir(build_id_dir: str) -> dict[str, str]:
    assert os.path.isabs(build_id_dir)
    out = {}
    for root, dirs, files in os.walk(build_id_dir):
        if len(files) != 0 and len(dirs) != 0:
            raise Exception(
                "%s is not a valid .build-id directory" % build_id_dir
            )
        for f in files:
            suffix = ".debug"
            if f.endswith(suffix):
                out[os.path.basename(root) + f[: -len(suffix)]] = os.path.join(
                    root, f
                )
    return out


def symlink(src: str, dst: str) -> None:
    assert os.path.isabs(src)
    if os.path.exists(dst):
        os.remove(dst)
    os.symlink(src, dst)


def hardlink(src: str, dst: str) -> None:
    src = os.path.realpath(src)
    if os.path.exists(dst):
        os.remove(dst)
    os.link(src, dst)


def mkdir(path: str) -> None:
    try:
        os.makedirs(path)
    except OSError as e:
        if e.errno != errno.EEXIST:
            raise e


def touch(path: str) -> None:
    if os.path.exists(path):
        os.utime(path, None)
    else:
        with open(path, "w"):
            return


def write_build_id_dir(
    build_id_dir: str, link_func: Callable[..., None], mods: dict[str, str]
) -> None:
    for build_id, path in mods.items():
        mkdir(os.path.join(build_id_dir, build_id[:2]))
        link_func(
            path,
            os.path.join(build_id_dir, build_id[:2], build_id[2:] + ".debug"),
        )


# if rel_to is None fix_path returns the absolute path. If rel_to
# is not None it turns the path into a relative path.
def fix_path(path: str, rel_to: str | None) -> str:
    assert os.path.isabs(path)
    assert rel_to is None or os.path.isabs(rel_to)
    if rel_to is None:
        return path
    return os.path.relpath(path, rel_to)


def write_ids_txt(ids_path: str, rel_to: str, mods: dict[str, str]) -> None:
    assert rel_to is None or os.path.isabs(rel_to)
    with open(ids_path, "w") as f:
        for build_id, path in sorted(mods.items()):
            path = fix_path(mods[build_id], rel_to)
            f.write("%s %s\n" % (build_id, path))


def main(unparsed_args: list[str] | None = None) -> None:
    ids_fmt = "ids.txt"
    build_id_fmt = ".build-id"
    symlink_mode = "symlink"
    hardlink_mode = "hardlink"

    parser = argparse.ArgumentParser(
        description="Convert between ids.txt and .build-id"
    )
    parser.add_argument(
        "-O",
        "--output-format",
        help="Sets the output format.",
        metavar="FMT",
        choices=[ids_fmt, build_id_fmt],
    )
    parser.add_argument(
        "--ids-rel-to-in",
        help="When reading ids.txt use paths relative to DIR",
        metavar="DIR",
    )
    parser.add_argument(
        "--ids-rel-to-out",
        help="When writing ids.txt use paths relative to DIR",
        metavar="DIR",
    )
    parser.add_argument(
        "--build-id-mode",
        help="When writing .build-id generate links of this type",
        metavar="MODE",
        choices=[symlink_mode, hardlink_mode],
        default=hardlink_mode,
    )
    parser.add_argument(
        "--stamp", help="Touch STAMP after finishing", metavar="STAMP"
    )
    parser.add_argument(
        "--input",
        action="append",
        help=".build-id directories or ids.txt files depending on the output format",
    )
    parser.add_argument("output")

    args = parser.parse_args(unparsed_args)

    input_paths_map = map(os.path.abspath, args.input)
    input_paths = list(
        filter(os.path.exists, input_paths_map)
    )  # conventionally ignore empty inputs
    input_dirs = list(filter(os.path.isdir, input_paths))
    if len(input_dirs) > 0:
        assert len(input_dirs) == len(
            input_paths
        ), "input formats cannot be mixed"
        in_fmt = build_id_fmt
    else:
        in_fmt = ids_fmt

    output_path = args.output
    rel_to_in = (
        os.path.abspath(args.ids_rel_to_in)
        if args.ids_rel_to_in is not None
        else None
    )
    rel_to_out = (
        os.path.abspath(args.ids_rel_to_out)
        if args.ids_rel_to_out is not None
        else None
    )

    if args.build_id_mode == symlink_mode:
        link_func = symlink
    else:
        link_func = hardlink

    mods = {}
    for input_path in input_paths:
        if in_fmt == ids_fmt:
            if rel_to_in is None:
                rel_to_in = os.path.abspath(os.path.dirname(input_path))
            mods.update(read_ids_txt(input_path, rel_to_in))
        else:
            mods.update(read_build_id_dir(input_path))

    if args.output_format == None:
        if in_fmt == ids_fmt:
            out_fmt = build_id_fmt
        else:
            out_fmt = ids_fmt
    else:
        out_fmt = args.output_format

    if out_fmt == ids_fmt:
        write_ids_txt(output_path, rel_to_out, mods)
    else:
        write_build_id_dir(output_path, link_func, mods)

    if args.stamp is not None:
        touch(args.stamp)


if __name__ == "__main__":
    main()
