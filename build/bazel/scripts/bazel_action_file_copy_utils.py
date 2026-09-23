# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Helper functions to deal with copying files"""
import errno
import filecmp
import os
import shutil
import stat
from pathlib import Path

import thread_pool_helpers
from build_utils import FilePath


def make_writable(p: FilePath) -> None:
    file_mode = os.stat(p).st_mode
    is_readonly = file_mode & stat.S_IWUSR == 0
    if is_readonly:
        os.chmod(p, file_mode | stat.S_IWUSR)


def copy_writable(src: FilePath, dst: FilePath) -> None:
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy2(src, dst)
    make_writable(dst)


def hardlink_or_copy_writable(
    src_path: str, dst_path: str, bazel_output_base_dir: str
) -> None:
    # Use lexists to make sure broken symlinks are removed as well.
    if os.path.lexists(dst_path):
        os.remove(dst_path)

    # See https://fxbug.dev/42072059 for context. This logic is kept here
    # to avoid incremental failures when performing copies across
    # different revisions of the Fuchsia checkout (e.g. when bisecting
    # or simply in CQ).
    #
    # If the file is writable, and not a directory, try to hard-link it
    # directly. Otherwise, or if hard-linking fails due to a cross-device
    # link, do a simple copy.
    do_copy = True
    file_mode = os.stat(src_path).st_mode
    is_src_readonly = file_mode & stat.S_IWUSR == 0
    if not is_src_readonly:
        try:
            os.makedirs(os.path.dirname(dst_path), exist_ok=True)

            # Get realpath of src_path to avoid symlink chains, which
            # os.link does not handle properly even follow_symlinks=True.
            #
            # NOTE: it is important to link to the final real file because
            # intermediate links can be temporary. For example, the
            # gn_targets repository is repopulated in every bazel_action, so
            # any links pointing to symlinks in gn_targets can be
            # invalidated during the build.
            os.link(os.path.realpath(src_path), dst_path)

            # Update timestamp to avoid Ninja no-op failures that can
            # happen because Bazel does not maintain consistent timestamps
            # in the execroot when sandboxing or remote builds are enabled.
            if os.path.realpath(src_path).startswith(
                os.path.abspath(bazel_output_base_dir)
            ):
                os.utime(dst_path)
            do_copy = False
        except OSError as e:
            if e.errno != errno.EXDEV:
                raise

    if do_copy:
        copy_writable(src_path, dst_path)


def copy_directory_if_changed(
    src_dir: FilePath, dst_dir: FilePath, tracked_files: list[FilePath]
) -> None:
    """Copy directory from |src_path| to |dst_path| if |tracked_files| have different mtimes.

    NOTE this function deliberately uses __mtime__, instead of content, of
    tracked_files to determine whether directories need a re-copy. This follows
    the convention many tools used in the build are using, where they use mtime
    of a file as a proxy for the freshness of a directory, because Ninja only
    understands timestamps.

    See http://b/365838961 for details.
    """
    assert os.path.isdir(
        src_dir
    ), "{} is not a dir, but copy dir is called.".format(src_dir)

    def all_tracked_files_unchanged(
        src_dir: FilePath, dst_dir: FilePath, tracked_files: list[FilePath]
    ) -> bool:
        """Use __mtime__ to determine whether any tracke file has changed.

        Returns true iff mtimes of tracked files are identical in src_dir and
        dst_dir.
        """
        for tracked_file in tracked_files:
            dst_tracked_file = os.path.join(dst_dir, tracked_file)
            if not os.path.exists(dst_tracked_file):
                return False
            src_tracked_file = os.path.join(src_dir, tracked_file)
            if os.path.getmtime(src_tracked_file) != os.path.getmtime(
                dst_tracked_file
            ):
                return False
        return True

    if all_tracked_files_unchanged(src_dir, dst_dir, tracked_files):
        return

    if os.path.lexists(dst_dir):
        rmtree_threaded(dst_dir)

    copy_directory_threaded(src_dir, dst_dir)


def rmtree_threaded(dirname: FilePath) -> None:
    """Uses a threadpool to delete all the files in a directory tree faster than shutil.rmtree().

    This is about 2x faster than using shutil.rmtree() on its own.
    """

    # Find all the files in the tree, from the bottom up so that the directories are emptied from
    # the bottom-up (the order they'll be deleted in.)
    files: list[str] = []
    for root, _, filenames in os.walk(dirname, topdown=False):
        files.extend([os.path.join(root, filename) for filename in filenames])

    # Delete all the files in one big threadpool
    thread_pool_helpers.map_threaded(os.remove, files)

    # Now delete all the (empty) direcctories
    shutil.rmtree(dirname)


def copy_directory_threaded(src_dir: FilePath, dst_dir: FilePath) -> None:
    directories: list[str] = []
    files: list[tuple[str, str]] = []
    for root, dirnames, filenames in os.walk(src_dir):
        relroot = os.path.relpath(root, src_dir)
        if relroot != ".":
            directories.append(os.path.join(dst_dir, relroot))
        files.extend(
            [
                (
                    os.path.join(src_dir, relroot, filename),
                    os.path.join(dst_dir, relroot, filename),
                )
                for filename in filenames
            ]
        )

    # Benchmarking confirmed that it's faster to create all the directories that are
    # needed, first, and then perform a multi-threaded copying of files, than it is
    # to use a thread pool copy per directory.
    thread_pool_helpers.map_threaded(os.makedirs, directories)
    thread_pool_helpers.starmap_threaded(copy_writable, files)


# filecmp uses a tiny buffer for comparisons, forcing it to a larger size will
# reduce the number of I/O operations and drastically speed it up (as much as 10x)
setattr(filecmp, "BUFSIZE", 256 * 1024)


def check_if_need_to_copy_file(args: tuple[Path, Path]) -> bool:
    """Check if the file copy given as a src,dst tuple needs to be performed.

    This compares the files and returns true if they need to be copied.
    """
    src_path, dst_path = args
    assert os.path.isfile(
        src_path
    ), "{} is not a file, but copy file is called.".format(src_path)

    # NOTE: For some reason, filecmp.cmp() will return True if
    # dst_path does not exist, even if src_path is not empty!?
    if os.path.exists(dst_path) and filecmp.cmp(
        src_path, dst_path, shallow=False
    ):
        return False
    return True


def write_file_if_changed(dst_path: FilePath, content: str) -> None:
    if os.path.exists(dst_path):
        with open(dst_path, "rt") as f:
            current_content = f.read()
        if current_content == content:
            return

    # Use lexists to make sure broken symlinks are removed as well.
    if os.path.lexists(dst_path):
        os.remove(dst_path)

    dst_dirname = os.path.dirname(dst_path)
    if dst_dirname:
        os.makedirs(dst_dirname, exist_ok=True)
    with open(dst_path, "wt") as f:
        f.write(content)
