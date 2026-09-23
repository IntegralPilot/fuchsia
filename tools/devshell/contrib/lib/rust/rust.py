#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2018 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import hashlib
import os
import platform
import re
from functools import lru_cache
from pathlib import Path

ROOT_PATH = Path(os.environ.get("FUCHSIA_DIR", ""))
FX_PATH = ROOT_PATH / "scripts" / "fx"
FUCHSIA_BUILD_DIR = Path(os.environ.get("FUCHSIA_BUILD_DIR", ""))
PREBUILT_DIR = ROOT_PATH / "prebuilt"
PREBUILT_THIRD_PARTY_DIR = PREBUILT_DIR / "third_party"
HOST_PLATFORM = (
    platform.system().lower().replace("darwin", "mac")
    + "-"
    + {"x86_64": "x64", "aarch64": "arm64", "arm64": "arm64"}[
        platform.machine()
    ]
)


class GnTarget:
    def __init__(self, gn_target, fuchsia_dir=None):
        build_label_regex = get_build_label_regex()
        match = re.match(build_label_regex, gn_target)
        if match is None:
            print(f"Invalid GN label '{gn_target}'")
            raise ValueError(gn_target)
        path, name, toolchain = match.group(1, 3, 5)

        if fuchsia_dir is None:
            fuchsia_dir = ROOT_PATH

        if path.startswith("//"):
            path = fuchsia_dir / Path(path[2:])
        else:
            path = Path(path).resolve()

        if name is None:
            name = path.name

        self.label_path = path.relative_to(fuchsia_dir)
        self.label_name = name
        self.explicit_toolchain = toolchain

    def __str__(self):
        return self.gn_target

    @property
    def ninja_target(self):
        """The canonical GN label of this target, minus the leading '//'."""
        return (
            str(self.label_path) + ":" + self.label_name + self.toolchain_suffix
        )

    def __actual_ninja_target(self):
        """The canonical GN label of the ".actual" target, minus the leading '//'."""
        label_name = self.label_name
        if not label_name.endswith(".actual"):
            label_name += ".actual"
        return str(self.label_path) + ":" + label_name + self.toolchain_suffix

    @property
    def gn_target(self):
        """The canonical GN label of this target, including the leading '//'."""
        return "//" + self.ninja_target

    @property
    def toolchain_suffix(self):
        """The GN path suffix for this target's toolchain, if it is not the default."""
        if (
            self.explicit_toolchain is None
            or "fuchsia" in self.explicit_toolchain
        ):
            return ""
        return "({})".format(self.explicit_toolchain)

    @property
    def src_path(self):
        """The path to the directory containing this target's BUILD.gn file."""
        return ROOT_PATH / self.label_path

    def gen_dir(self, build_dir=None):
        """The path to the directory containing this target's generated files."""
        tc = self.explicit_toolchain
        build_dir = build_dir or FUCHSIA_BUILD_DIR
        default = default_toolchain(build_dir)
        return (
            build_dir
            / (tc.split(":")[-1] if tc and tc != default else "")
            / "gen"
            / self.label_path
        )

    def manifest_path(self, build_dir=None):
        """The path to Cargo.toml for this target."""
        if build_dir is None:
            build_dir = FUCHSIA_BUILD_DIR

        hashed_gn_path = hashlib.sha1(
            self.__actual_ninja_target().encode("utf-8")
        ).hexdigest()
        return Path(build_dir) / "cargo" / hashed_gn_path / "Cargo.toml"


@lru_cache
def get_build_label_regex():
    """get a regular expression for parsing build target labels"""
    # [\w-] is a valid GN name. We also accept '/' and '.' in paths.
    # For the toolchain suffix, we take the whole label name at once, so we allow ':'.
    build_label_regex = (
        r"([\w/.-]*)" + r"(:([+\w.-]+))?" + r"(\(([\w./:+-]+)\))?$"
    )
    # Here are some examples of matching labels
    assert (
        re.match(
            build_label_regex,
            "//third_party/rust_crates:zstd-v0_11_2+zstd_1_5_2.rustdoc(//build/toolchain/fuchsia:x64)",
        )
        is not None
    )
    assert re.match(build_label_regex, "//src/lib/fuchsia-async") is not None
    assert (
        re.match(build_label_regex, "//build/rust/tests:clippy_test")
        is not None
    )
    return re.compile(build_label_regex)


@lru_cache
def default_toolchain(build_dir):
    return open(build_dir / "default_toolchain_name.txt").read()
