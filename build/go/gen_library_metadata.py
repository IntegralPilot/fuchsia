#!/usr/bin/env fuchsia-vendored-python
# Copyright 2018 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys

FUCHSIA_MODULE = "go.fuchsia.dev/fuchsia"


class Source(object):
    def __init__(self, name, path, file):
        self.name = name
        self.path = path
        self.file = file

    def __str__(self):
        return "%s[%s]" % (self.name, self.path)

    def __hash__(self):
        return hash((self.name, self.path))

    def __eq__(self, other):
        return self.name == other.name and self.path == other.path


def get_sources(dep_files, extra_sources=None):
    # Aggregate source data from dependencies.
    sources = set()
    if extra_sources:
        sources.update(extra_sources)
    for dep in dep_files:
        with open(dep, "r") as dep_file:
            for name, path in json.load(dep_file)["sources"].items():
                sources.add(Source(name, path, dep))

    # Verify duplicates.
    sources_by_name = {}
    for src in sources:
        sources_by_name.setdefault(src.name, []).append(src)
    for name, srcs in sources_by_name.items():
        if len(srcs) <= 1:
            continue
        print('Error: source "%s" has multiple paths.' % name)
        for src in srcs:
            print(" - %s (%s)" % (src.path, src.file))
        raise Exception("Could not aggregate sources")

    return {s.name: s.path for s in sources}


def main():
    parser = argparse.ArgumentParser()
    name_group = parser.add_mutually_exclusive_group(required=True)
    name_group.add_argument("--name", help="Name of the current library")
    name_group.add_argument(
        "--name-file",
        help="Path to a file containing the name of the current library",
    )
    parser.add_argument(
        "--root-build-dir",
        help="Path to the root build directory",
        required=True,
    )
    parser.add_argument(
        "--fuchsia-source-dir",
        help="Path to the Fuchsia source directory (autodetected)",
    )
    parser.add_argument(
        "--source-dir",
        help="Path to the library's source directory",
        required=True,
    )
    sources_group = parser.add_mutually_exclusive_group(required=True)
    sources_group.add_argument(
        "--sources", help="List of source files", nargs="*"
    )
    parser.add_argument(
        "--output", help="Path to the file to generate", required=True
    )
    parser.add_argument(
        "--deps", help="Dependencies of the current library", nargs="*"
    )
    args = parser.parse_args()
    if args.name:
        name = args.name
    elif args.name_file:
        with open(args.name_file, "r") as name_file:
            name = name_file.read()

    build_dir = os.path.abspath(args.root_build_dir)

    # Find source_root from library source_dir, i.e. do not assume
    # that the build directory is two levels down the source root
    if args.fuchsia_source_dir:
        source_root = args.fuchsia_source_dir
    else:
        source_root = build_dir
        while not os.path.exists(os.path.join(source_root, ".jiri_manifest")):
            source_root = os.path.dirname(source_root)
            if source_root in ("", "/"):
                parser.error("Cannot find Fuchsia source directory!")

    third_party_dir = os.path.join(source_root, "third_party")

    # For Fuchsia sources, the declared package name must correspond to the
    # source directory so that raw `go` commands (e.g., as an IDE would use) can
    # resolve the source path based on the package name.
    # TODO(olivernewman): Stop exempting package names that don't start with
    # `FUCHSIA_MODULE`; all packages should use absolute names that start with
    # the module name.
    if name.startswith(FUCHSIA_MODULE) and not os.path.abspath(
        args.source_dir
    ).startswith((build_dir, third_party_dir)):
        expected_name = (
            FUCHSIA_MODULE
            + "/"
            + os.path.relpath(args.source_dir, source_root).replace(
                os.path.sep, "/"
            )
        )
        if name not in (expected_name, expected_name + "/..."):
            raise ValueError(
                f"go_library name must correspond to the source dir: "
                f"got {name!r}, expected {expected_name!r}"
            )

    current_sources = []
    for source in args.sources:
        p = os.path.join(args.source_dir, source)
        # Explicit sources must be files.
        if not os.path.isfile(p):
            raise ValueError(f"Source {p} is not a file")
        current_sources.append(
            Source(os.path.join(name, source), p, args.output)
        )
    if not name.endswith("/..."):
        # Get the common subdirectory of all sources, which is necessary to
        # determine the Go package name for these sources.
        dirs = set(
            os.path.dirname(src) for src in args.sources if src.endswith(".go")
        )
        if len(dirs) > 1:
            raise ValueError(
                f"Sources are from multiple directories {dirs}, "
                f"this is not supported by go_library"
            )
        subdir = list(dirs)[0]
        name = os.path.join(name, subdir)
        source_dir = os.path.join(args.source_dir, subdir)

        go_sources = {
            os.path.basename(f) for f in args.sources if f.endswith(".go")
        }

        # Require all non-generated Go files to be listed as sources.
        if not os.path.abspath(source_dir).startswith(build_dir):
            # TODO: Use `glob.glob("*.go", root_dir=source_dir)`
            # instead of os.listdir after upgrading to Python 3.10.
            go_files = {
                f
                for f in os.listdir(source_dir)
                if f.endswith(".go") and not f.endswith("_test.go")
            }
            missing = go_files - go_sources
            if missing:
                raise ValueError(
                    f"go_library requires that all non-test Go files in "
                    f"source_dir be listed as sources, but the following "
                    f"files are missing from sources for target {name}:"
                    f' {", ".join(sorted(missing))}'
                )
    result = get_sources(args.deps, extra_sources=current_sources)
    with open(args.output, "w") as output_file:
        json.dump(
            {
                "package": name,
                "sources": result,
            },
            output_file,
            indent=2,
            sort_keys=True,
        )


if __name__ == "__main__":
    sys.exit(main())
