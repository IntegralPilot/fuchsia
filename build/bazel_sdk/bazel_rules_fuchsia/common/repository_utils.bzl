# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities for repository rules."""

_FUCHSIA_HOST_OS_MAP = {
    "windows": "win",
}

def get_fuchsia_host_os(repo_ctx):
    """Return host os string according to Fuchsia conventions.

    Args:
      repo_ctx: repository context.
    Returns:
      A string describing the current host system (e.g. 'linux' or 'win').
    """
    host_os = repo_ctx.os.name.split(" ")[0]
    return _FUCHSIA_HOST_OS_MAP.get(host_os, host_os)

_FUCHSIA_HOST_ARCH_MAP = {
    "x86_64": "x64",
    "amd64": "x64",
    "aarch64": "arm64",
}

def get_fuchsia_host_arch(repo_ctx):
    """Return host architecture string according to Fuchsia conventions.

    Args:
      repo_ctx: repository context.
    Returns:
      A string describing the current host cpu (e.g. 'x64' or 'arm64').
    """
    host_arch = repo_ctx.os.arch
    return _FUCHSIA_HOST_ARCH_MAP.get(host_arch, host_arch)

def bazel_major_version_is_at_least(repo_ctx, min_major_version):
    """Return True if this version of Bazel is at least 7."""
    version = native.bazel_version

    # Expected format is either an empty string, to denote a development version,
    # or 3 dot-separated integers (e.g. "<major>.<minor>.<patch>"), optionally
    # followed by -<suffix> where <suffix> could be "rc1", "rc2" for pre-release
    # candidates, and otherwise a vendor-specific suffix.
    if not version:
        # An empty string denotes a development version.
        return True

    # To support Bazel 10+ in the future, get the first decimal integers.
    major = ""
    for i in range(len(version)):
        c = version[i]
        if c >= "0" and c <= "9":
            major += c
        else:
            break

    return int(major) >= min_major_version
