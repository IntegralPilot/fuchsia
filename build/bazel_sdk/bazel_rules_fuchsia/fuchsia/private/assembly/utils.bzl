# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utility functions used by multiple bazel rules and macros."""

load("@bazel_skylib//lib:paths.bzl", "paths")

def select_root_dir_with_file(files, file):
    """Finds the top-most directory that has a direct file with the name `file`

    Args:
        files: A list of files.
        file: The name of a file that should be found in the return directory.

    Returns:
        The top-most directory that contains the `file`.
    """
    return paths.dirname(select_single_file(files, file).path)

def select_root_dir(files):
    """Finds the top-most directory in a set of files.

    Args:
      files: A list of files.

    Returns:
      The top-most directory.
    """
    shortest = paths.dirname(files[0].path)
    for file in files:
        directory = paths.dirname(file.path)
        if len(directory) < len(shortest):
            shortest = directory
    return shortest

def select_single_file(files, basename, error_footer = ""):
    """Finds a single file with a given basename. Multiple matches will fail.

    Args:
      files: A list of files.
      basename: The basename of the desired file.
      error_footer: Optionally adds a non-generic error message footer.

    Returns:
      The single file matching the basename.
      It's guaranteed that exactly one file matches that basename.
    """
    matching_files = [file for file in files if file.basename == basename]

    if not matching_files:
        NO_MATCHING_FILE = "\n\nCould not find {} in {}.\n{}".format(
            basename,
            files,
            error_footer,
        ).rstrip() + "\n\n"
        fail(NO_MATCHING_FILE)

    if len(matching_files) > 1:
        AMBIGUOUS_FILE_MATCH = "\n\nToo many matches of {} in {}. (Multiple matches are not allowed).\n{}".format(
            basename,
            files,
            error_footer,
        ).rstrip() + "\n\n"
        fail(AMBIGUOUS_FILE_MATCH)

    return matching_files[0]

def select_multiple_files(files, basename, error_footer = ""):
    """Finds all files that match the given basename. Zero matches will fail.

    Args:
      files: A list of files.
      basename: The basename of the desired files.
      error_footer: Optionally adds a non-generic error message footer.

    Returns:
      A list of files matching the basename.
      It's guaranteed that this list is non-empty.
    """
    matching_files = [file for file in files if file.basename == basename]

    if not matching_files:
        # Assign error string to variable for a cleaner stack trace.
        NO_MATCHING_FILES = "\n\nCould not find any {} in {}.\n{}".format(
            basename,
            files,
            error_footer,
        ).rstrip() + "\n\n"
        fail(NO_MATCHING_FILES)

    return matching_files
