# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Python Types that are shared across different parts of the assembly types"""
import os
import shutil
from collections.abc import Iterable
from dataclasses import dataclass
from functools import total_ordering
from typing import Any, TextIO

import serialization

__all__ = ["FileEntry", "FilePath", "fast_copy", "fast_copy_makedirs"]

FilePath = str | os.PathLike[Any]


# TODO(https://fxbug.dev/42170926) Move to python module at //build/python/modules/file_entry
@total_ordering
@dataclass
@serialization.serialize_dict
class FileEntry:
    """FileEntry Class

    This is a source_path=destination_path mapping type
    """

    # TODO(https://fxbug.dev/42180921) Mark these fields to `kw_only=True` after switching
    #   to python 3.10 or later.
    source: FilePath
    destination: FilePath

    def __hash__(self) -> int:
        return hash((self.destination, self.source))

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, self.__class__):
            return (
                self.source == other.source
                and self.destination == other.destination
            )
        else:
            return False

    def __lt__(self, other: Any) -> bool:
        if not isinstance(other, FileEntry):
            raise ValueError("other is not a FileEntry")
        return (self.source, self.destination) < (
            other.source,
            other.destination,
        )

    def __repr__(self) -> str:
        result = "FileEntry{{ source: {}, destination: {} }}".format(
            self.source, self.destination
        )
        return result

    # TODO(https://fxbug.dev/42170926) Move to python module at //build/python/modules/fini_manfest
    @staticmethod
    def write_fini_manifest(
        entries: Iterable["FileEntry"], file: TextIO
    ) -> None:
        """Write out the FileEntry instances into a FINI manifest.

        The fini manifest is in the format of::

            dest_path=source_path

        If the `rebase` path is supplied, all destination paths are made relative to it.
        """
        for entry in sorted(entries):
            dst = entry.destination
            src = entry.source
            file.write("{}={}\n".format(dst, src))


def fast_copy(src: FilePath, dst: FilePath) -> FilePath:
    """A wrapper around os and os.path fns to copy a file with a better error message than
    shutil.copy2() has.

    This used to use os.link() to perform hard links instead of copies, but
    with the switch to bazel, we're starting to see issues with hard links and so we're
    switching back to "slow copies".
    """

    # This is a cleaner error for this case than we'd get if we threw an exception
    # from copy2().
    if not os.path.exists(src):
        raise FileNotFoundError(f"Source file {src} does not exist.")

    real_src_path = os.path.realpath(src)
    shutil.copy2(real_src_path, dst)

    # Return src so it can be added to deps
    return src


def fast_copy_makedirs(src: FilePath, dst: FilePath) -> FilePath:
    """Run `fast_copy`, making the destination directory if it doesn't exist"""
    # Create parents if they don't exist
    os.makedirs(os.path.dirname(dst), exist_ok=True)

    # Copy the file from the source to the destination
    return fast_copy(src, dst)
