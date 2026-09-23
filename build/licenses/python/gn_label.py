#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utilities for working with GnLabel strings."""

import dataclasses
import logging
import os
from pathlib import Path
from typing import Any


@dataclasses.dataclass(frozen=True)
class GnLabel:
    """Utility for handling Gn target labels"""

    """The original GN label string, e.g. `//foo/bar:baz(//toolchain)`"""
    gn_str: str
    """The path part of the label, e.g. `foo/bar` in `//foo/bar:baz`"""
    # path: Path = dataclasses.field(hash=False, compare=False)
    path_str: str = dataclasses.field(hash=False, compare=False)
    """The name part of the label, e.g. `baz` in `//foo/bar:baz` or `bar` for `//foo/bar`"""
    name: str = dataclasses.field(hash=False, compare=False)
    """Whether the label has a local name, e.g. True for `//foo/bar:baz` but false for `//foo/bar/baz`"""
    is_local_name: bool = dataclasses.field(hash=False, compare=False)
    """The toolchain part of the label, e.g. `//toolchain` in `//foo/bar(//toolchain)`"""
    toolchain: "GnLabel|None" = dataclasses.field(hash=False, compare=False)
    """The URL pointing to this GN label, e.g. https://cs.opensource.google/fuchsia/..."""
    url_str: str = dataclasses.field(hash=False, compare=False)

    @staticmethod
    def from_str(original_str: str, url: str = "") -> "GnLabel":
        """Constructs a GnLabel instance from a GN target label string"""
        assert original_str.startswith(
            "//"
        ), f"label must start with // but got {original_str}"

        assert not original_str.startswith(
            "///"
        ), f"label can't start with /// but got {original_str}"

        label = original_str
        toolchain = None
        # Extract toolchain part
        if original_str.endswith(")"):
            toolchain_begin = original_str.rfind("(")
            if toolchain_begin != -1:
                toolchain_str = original_str[toolchain_begin + 1 : -1]
                toolchain = GnLabel.from_str(toolchain_str)
                label = original_str[0:toolchain_begin]

        path, colon, name = label[2:].partition(":")
        if ".." in path:
            path = GnLabel._resolve_dot_dot(path)

        if colon:
            is_local_name = True
            gn_str = f"//{path}:{name}"
        else:
            name = os.path.basename(path)
            is_local_name = False
            gn_str = f"//{path}"

        if toolchain:
            gn_str += f"({toolchain.gn_str})"

        return GnLabel(
            gn_str=gn_str,
            name=name,
            is_local_name=is_local_name,
            path_str=path,
            url_str=url,
            toolchain=toolchain,
        )

    @staticmethod
    def from_path(path: Path | str, url: str = "") -> "GnLabel":
        """Constructs a GnLabel instance from a Path object."""
        if isinstance(path, Path):
            if path.name == "" and path.parent.name == "":
                return GnLabel.from_str("//", url=url)
            return GnLabel.from_str(f"//{path}", url=url)
        elif isinstance(path, str):
            if (
                os.path.basename(path) == ""
                and os.path.basename(os.path.dirname(path)) == ""
            ):
                return GnLabel.from_str("//", url=url)
            return GnLabel.from_str(f"//{path}", url=url)
        else:
            assert False, f"Expected path of type Path but got {type(path)}"

    @staticmethod
    def check_type(other: Any) -> "GnLabel":
        """Asserts that `other` is of type GnLabel"""
        assert isinstance(
            other, GnLabel
        ), f"{other} type {type(other)} is not {GnLabel}"
        return other

    @staticmethod
    def check_types_in_list(list: list["GnLabel"]) -> list["GnLabel"]:
        """Asserts that all values in `list` are of type GnLabel"""
        for v in list:
            GnLabel.check_type(v)
        return list

    def parent_label(self) -> "GnLabel":
        """Returns //foo/bar for //foo/bar/baz and //foo/bar:baz"""
        assert self.has_parent_label(), f"{self} has no parent label"
        if not self.is_local_name and self.name == os.path.basename(
            self.path_str
        ):
            # Return //foo for //foo/bar
            return GnLabel.from_path(os.path.dirname(self.path_str))
        else:
            # Return //foo/bar for //foo/bar:bar and //foo/bar:baz
            return GnLabel.from_path(self.path_str)

    def has_parent_label(self) -> bool:
        return self.gn_str != "//"

    def without_toolchain(self) -> "GnLabel":
        """Removes the toolchain part of the label"""
        if self.toolchain:
            return GnLabel.from_str(self.gn_str.split("(", maxsplit=1)[0])
        else:
            return self

    def ensure_toolchain(self, toolchain: "GnLabel") -> "GnLabel":
        """Sets a toolchain if there is none already"""
        if self.toolchain:
            return self
        else:
            return dataclasses.replace(
                self, toolchain=toolchain, gn_str=f"{self.gn_str}({toolchain})"
            )

    def rebased_path(self, base_dir: Path) -> Path:
        """Returns package_path rebased to a given base_dir."""
        return base_dir / self.path_str

    def url(self) -> str:
        """Returns URL pointing to this GN label."""
        if self.url_str:
            return self.url_str
        return f"https://cs.opensource.google/fuchsia/fuchsia/+/main:{self.path_str}"

    def is_host_target(self) -> bool:
        # Some host-tools do not have a toolchain.
        # Add them here to ensure we skip collecting those licenses.
        extra_host_targets = [
            "prebuilt/third_party/rust/",
            "prebuilt/third_party/python3/",
        ]
        if any(substring in self.path_str for substring in extra_host_targets):
            logging.debug(
                "Target %s is a host-tool even though the toolchain (%s) does not start with 'host_'",
                self.path_str,
                self.toolchain.name if self.toolchain else "",
            )
            return True
        return bool(self.toolchain and self.toolchain.name.startswith("host_"))

    def is_3rd_party(self) -> bool:
        return "third_party" in self.gn_str or "thirdparty" in self.gn_str

    def is_prebuilt(self) -> bool:
        return "prebuilt" in self.gn_str

    def is_3p_rust_crate(self) -> bool:
        return self.gn_str.startswith("//third_party/rust_crates:")

    def is_3p_golib(self) -> bool:
        return self.gn_str.startswith("//third_party/golibs:")

    def is_spdx_json_document(self) -> bool:
        return self.name.endswith(".spdx.json")

    def create_child_from_str(
        self, child_path_str: str, url: str = ""
    ) -> "GnLabel":
        """Create a GnLabel relative to this label from a child path GN string"""
        if child_path_str.startswith("//"):
            return GnLabel.from_str(child_path_str)
        elif child_path_str.startswith(":"):
            assert (
                not self.is_local_name
            ), f"Can't apply {child_path_str} to {self} because both have :"
            return GnLabel.from_str(f"//{self.path_str}:{child_path_str[1:]}")
        elif child_path_str.startswith("../"):
            parent_path = (
                self.parent_label().path_str
                if self.is_local_name
                else self.path_str
            )
            return GnLabel.from_path(
                os.path.join(parent_path, child_path_str), url=url
            )
        else:
            return GnLabel.from_path(
                os.path.join(self.path_str, child_path_str), url=url
            )

    def __str__(self) -> str:
        return self.gn_str

    def __repr__(self) -> str:
        return f"{self.__class__.__name__}({self.gn_str})"

    def __gt__(self, other: "GnLabel") -> bool:
        return self.gn_str > other.gn_str

    @staticmethod
    def _resolve_dot_dot(path: str) -> str:
        """Resolves .. elements in a path string"""
        assert "//" not in path
        input_parts = path.split("/")
        output_parts = []
        for part in input_parts:
            if part != "..":
                output_parts.append(part)
            else:
                assert (
                    len(output_parts) > 0
                ), f".. goes back beyond the base path: {path}"
                output_parts.pop()
        return "/".join(output_parts)
