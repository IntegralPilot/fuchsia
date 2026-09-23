#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
The script to generate CMakeLists.txt file for SDK packages.
"""

import argparse
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

CMAKE_TEMPLATE = """
cmake_minimum_required(VERSION 3.20)

project(FuchsiaSDK VERSION {sdk_version} LANGUAGES C CXX)

set(FUCHSIA_SDK_DIR "{fuchsia_sdk_dir}" CACHE PATH "")
"""

INTERFACE_LIB_TEMPLATE = """
add_library({name} INTERFACE)

set({name}_PUBLIC_HEADERS
  {headers}
)

set_target_properties({name} PROPERTIES PUBLIC_HEADER "${{{name}_PUBLIC_HEADERS}}")

target_include_directories({name}
  PUBLIC INTERFACE
    {include_dir}
)
"""

PKG_LIB_TEMPLATE = """
add_library({name} STATIC
  {sources}
)

set_target_properties({name} PROPERTIES POSITION_INDEPENDENT_CODE ON)

set({name}_PUBLIC_HEADERS
  {headers}
)

set_target_properties({name} PROPERTIES PUBLIC_HEADER "${{{name}_PUBLIC_HEADERS}}")

target_include_directories(zx
  PUBLIC
    {include_dir}
)

target_link_libraries({name}
  PUBLIC
    {dependencies}
)
"""


@dataclass
class SDKManifest:
    name: str
    sources: list[str]
    include_dir: str
    headers: list[str]
    deps: list[str]


def read_manifest(path):
    with open(path, encoding="utf-8") as f:
        manifest = json.load(f)
        return SDKManifest(
            name=manifest["name"],
            sources=manifest["sources"],
            include_dir=manifest["include_dir"],
            headers=manifest["headers"],
            deps=manifest["deps"],
        )


def wrap_sdk_dir(file_list):
    return "\n  ".join(f'"${{FUCHSIA_SDK_DIR}}/{item}"' for item in file_list)


def generate_library(pkg_manifest):
    manifest = read_manifest(pkg_manifest)

    sources = wrap_sdk_dir(manifest.sources)
    include_dir = f'"${{FUCHSIA_SDK_DIR}}/{manifest.include_dir}"'
    headers = wrap_sdk_dir(manifest.headers)
    dependencies = "\n    ".join(f"{item}" for item in manifest.deps)
    return (
        PKG_LIB_TEMPLATE.format(
            name=manifest.name,
            sources=sources,
            headers=headers,
            include_dir=include_dir,
            dependencies=dependencies,
        ),
        manifest.deps,
    )


def generate_deps(pkg_manifest):
    manifest = read_manifest(pkg_manifest)
    assert len(manifest.sources) == 0
    headers = wrap_sdk_dir(manifest.headers).strip()
    return INTERFACE_LIB_TEMPLATE.format(
        name=manifest.name,
        headers=headers,
        include_dir=f'"${{FUCHSIA_SDK_DIR}}/{manifest.include_dir}"',
    )


def generate_cmake(sdk_dir, enabled_pkgs):
    sdk_dir = Path(os.path.abspath(sdk_dir))
    meta_manifest_path = sdk_dir.joinpath("meta", "manifest.json")
    with open(meta_manifest_path, encoding="utf-8") as f:
        meta_manifest = json.load(f)
    sdk_version = meta_manifest["id"]
    pkg_manifests = {}
    for item in meta_manifest["parts"]:
        meta_path = item["meta"]
        if item["type"] != "cc_source_library":
            continue
        # path separator in SDK's json is always "/"
        meta_list = meta_path.split("/")
        if meta_list[0] != "pkg":
            continue
        pkg_manifests[meta_list[1]] = meta_path

    pkg_libs = ""
    dep_list = []
    for pkg in enabled_pkgs:
        assert pkg in pkg_manifests
        lib_declaration, deps = generate_library(
            sdk_dir.joinpath(pkg_manifests[pkg])
        )
        pkg_libs += lib_declaration
        dep_list.extend(deps)

    interface_libs = ""
    for dep in dep_list:
        assert dep in pkg_manifests
        dep_declaration = generate_deps(sdk_dir.joinpath(pkg_manifests[dep]))
        interface_libs += dep_declaration

    return (
        CMAKE_TEMPLATE.format(fuchsia_sdk_dir=sdk_dir, sdk_version=sdk_version)
        + interface_libs
        + pkg_libs
    )


def main():
    parser = argparse.ArgumentParser()

    parser.add_argument(
        "--sdk-dir",
        help="the directory of the fuchsia SDK",
        required=True,
    )

    parser.add_argument(
        "--output",
        type=argparse.FileType("w"),
        default="-",
        help="output path",
    )

    parser.add_argument(
        "pkgs", help="the name of library that should be built", nargs="+"
    )

    args = parser.parse_args()
    content = generate_cmake(args.sdk_dir, args.pkgs)

    args.output.write(content)
    return 0


if __name__ == "__main__":
    sys.exit(main())
