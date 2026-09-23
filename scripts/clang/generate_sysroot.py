#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import glob
import json
import os
import shutil
import subprocess
import sys

"""
The sdk is organized as such:

//arch/
//arch/x64/...
//arch/{other_architectures}/...
//pkg/
//pkg/{libs}/
//pkg/sysroot/
//pkg/sysroot/meta.json

This script will create a new arch/ directory (--arch-name) by copying the
layout described in //pkg/sysroot/meta.json for an existing architecture.
This involves first copying headers which will be no different, and also
all shared objects in //pkg/arch/{existing_arch}/sysroot/lib. Most of these
shared objects are actually just linker scripts so they can be safely copied.
The other shared objects will be later replaced by //pkg/sysroot/*.ifs files,
currently this is libc.ifs and zircon.ifs. Scrt1.o is also replaced.
//pkg/arch/{new_arch}/lib is populated from shared objects created from
//pkg/*/*.ifs files (excluding //pkg/sysroot).
"""

TARGET_TO_TRIPLE = {
    "x64": "x86_64-unknown-fuchsia",
    "arm64": "aarch64-unknown-fuchsia",
    "riscv64": "riscv64-unknown-fuchsia",
}


def get_filename_from_ifs(ifs_file):
    ifs_file = os.path.basename(ifs_file)
    # This is necessary because libc.ifs is not called c.ifs, which would be the
    # conventional naming scheme for other libraries.
    name = os.path.splitext(ifs_file)[0]
    if not name.startswith("lib"):
        name = "lib" + name
    return name + ".so"


class SysrootGenerator:
    def _copy_files(self, other_arch, other_paths, meta_json_key):
        for src_path in other_paths:
            path = src_path.replace(other_arch, self.arch_name, 1)
            os.makedirs(
                os.path.join(self.sdk_dir, os.path.dirname(path)), exist_ok=True
            )
            shutil.copyfile(
                os.path.join(self.sdk_dir, src_path),
                os.path.join(self.sdk_dir, path),
            )
            self.sysroot_meta["versions"][self.arch_name][meta_json_key].append(
                path
            )

    def _copy_headers(self, other_arch, other_headers):
        self._copy_files(other_arch, other_headers, "headers")

    def _copy_link_libs(self, other_arch, other_link_libs):
        self._copy_files(other_arch, other_link_libs, "link_libs")
        self._make_Scrt1()

    def _make_Scrt1(self):
        # For bringing up new architectures executables won't be run anyway, this just creates
        # an empty Scrt1.o.
        Scrt1 = os.path.join(
            self.sdk_dir, "arch", self.arch_name, "sysroot", "lib", "Scrt1.o"
        )
        open(Scrt1, "w")

    def __init__(self, sdk_dir, arch_name):
        self.sdk_dir = sdk_dir
        self.arch_name = arch_name

    def _create_from_dir_structure(self):
        other_arch = os.listdir(os.path.join(self.sdk_dir, "arch"))[0]

        def make_src_dst(dir):
            return os.path.join(
                self.sdk_dir, "arch", other_arch, "sysroot", dir
            ), os.path.join(
                self.sdk_dir, "arch", self.arch_name, "sysroot", dir
            )

        src, dst = make_src_dst("include")
        shutil.copytree(src, dst)

        src, dst = make_src_dst("lib")
        shutil.copytree(src, dst)
        self._make_Scrt1()

    def _create_from_meta_json(self, meta_json):
        with open(meta_json) as f:
            self.sysroot_meta = json.load(f)
            other_arch, other_version = next(
                iter(self.sysroot_meta["versions"].items())
            )
            other_headers, other_link_libs = (
                other_version["headers"],
                other_version["link_libs"],
            )
            self.sysroot_meta["versions"][self.arch_name] = {
                "debug_libs": [],
                "dist_dir": f"arch/{self.arch_name}/sysroot",
                "dist_libs": [],
                "headers": [],
                "include_dir": f"arch/{self.arch_name}/sysroot/include",
                "link_libs": [],
                "root": f"arch/{self.arch_name}/sysroot",
            }
        self._copy_headers(other_arch, other_headers)
        self._copy_link_libs(other_arch, other_link_libs)

    def create(self):
        meta_json = os.path.join(self.sdk_dir, "pkg", "sysroot", "meta.json")
        if os.path.exists(meta_json):
            self.emit_meta_json = True
            self._create_from_meta_json(meta_json)
        else:
            pkg_sysroot = os.path.join(self.sdk_dir, "pkg", "sysroot")
            self.emit_meta_json = False
            self.sysroot_meta = {
                "ifs_files": glob.glob("*.ifs", root_dir=pkg_sysroot)
            }
            self._create_from_dir_structure()

    def add_stubs(self, create_stub):
        for ifs_file in self.sysroot_meta["ifs_files"]:
            source = os.path.join(self.sdk_dir, "pkg", "sysroot", ifs_file)
            dest = os.path.join(
                self.sdk_dir,
                "arch",
                self.arch_name,
                "sysroot",
                "lib",
                get_filename_from_ifs(ifs_file),
            )
            create_stub(source, dest)

    def finish(self):
        if not self.emit_meta_json:
            return
        with open(
            os.path.join(self.sdk_dir, "pkg", "sysroot", "meta.json"), "w"
        ) as f:
            json.dump(self.sysroot_meta, f)


def create_libs(sdk_dir, arch_name, create_stub):
    dirs = [
        f
        for f in os.listdir(os.path.join(sdk_dir, "pkg"))
        if os.path.basename(f) != "sysroot"
    ]
    for dir in dirs:
        ifs_file = os.path.join(sdk_dir, "pkg", dir, f"{dir}.ifs")
        if os.path.exists(ifs_file):
            os.makedirs(
                os.path.join(sdk_dir, "arch", arch_name, "lib"), exist_ok=True
            )
            create_stub(
                ifs_file,
                os.path.join(
                    sdk_dir,
                    "arch",
                    arch_name,
                    "lib",
                    get_filename_from_ifs(ifs_file),
                ),
            )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--sdk-dir",
        required=True,
        help="Path to sdk, should have arch/ and pkg/ at top level",
    )
    parser.add_argument(
        "--arch", required=True, help="ifs target to use for generating stubs"
    )
    parser.add_argument("--ifs-path", default="llvm-ifs")
    args = parser.parse_args()

    if os.path.exists(os.path.join(args.sdk_dir, "arch", args.arch)):
        print(f"arch/{args.arch} already exists!")
        return 1

    def write_stub(src, dest):
        subprocess.check_call(
            [
                args.ifs_path,
                "--target",
                TARGET_TO_TRIPLE[args.arch],
                f"--output-elf={dest}",
                src,
            ]
        )

    sysroot_creator = SysrootGenerator(args.sdk_dir, args.arch)
    sysroot_creator.create()
    sysroot_creator.add_stubs(write_stub)
    sysroot_creator.finish()

    # These are libs outside of the sysroot like fdio, etc.
    create_libs(args.sdk_dir, args.arch, write_stub)

    return 0


if __name__ == "__main__":
    sys.exit(main())
