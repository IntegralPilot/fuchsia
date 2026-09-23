# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Definitions related to the prebuilt Linux sysroot used by Fuchsia."""

# Location of Linux sysroot in the platform build.
# Should we move this to a repository to be shared with the SDK?
_linux_sysroot = "prebuilt/third_party/sysroot/linux"

# List of all CPU architectures present in the current Linux sysroot.
# Each linux_sysroot_${arch} filegroup() should exclude those that do not
# correspond to its architecture, to avoid polluting the sandbox with
# un-necessary files.
_linux_sysroot_archs = [
    "aarch64",
    "arm",
    "i386",
    "x86_64",
]

def linux_sysroot_headers(name, sysroot_path = _linux_sysroot):
    """Generates a filegroup() that exposes headers from a Linux sysroot.

    No attempt is made to filter headers based on CPU architecture.

    Args:
        name: name of the generated filegroup().
        sysroot_path: path to the top-level sysroot directory.
    """
    native.filegroup(
        name = name,
        srcs = native.glob([sysroot_path + "/usr/include/**"]),
        visibility = ["//visibility:public"],
    )

def linux_sysroot_libs(name, sysroot_arch, sysroot_path = _linux_sysroot):
    """Generates a filegroup() that exposes link-time Linux sysroot libraries.

    This only exposes the libraries for a specific CPU architecture.

    Args:
        name: name of the generated filegroup().
        sysroot_arch: target CPU architecture for the filegroup, must be
            one of the values listed in _linux_sysroot_archs above.
        sysroot_path: path to the top-level sysroot directory.
    """
    if sysroot_arch not in _linux_sysroot_archs:
        fail("The sysroot_arch parameter must be one of: %s" % _linux_sysroot_archs)

    # Runtime libraries are scatterred in several places.
    glob_patterns = [
        # This file is the dynamic linker configuration script. Not sure whether this is really required.
        sysroot_path + "/etc/ld.so.conf.d/{arch}-linux-gnu.conf",
        # This directory contains low-level system libraries related to the C library and dynamic linker
        sysroot_path + "/lib/{arch}-linux-gnu/**",
        # This directory contains higher-level system libraries (e.g. libX11.so) and crt1.o / crti.o / crtn.o
        sysroot_path + "/usr/lib/{arch}-linux-gnu/**",
    ]

    # The location of the dynamic linker is different for x64 compared to other architectures.
    if sysroot_arch == "x86_64":
        glob_patterns.append(sysroot_path + "/lib64/ld-linux-x86-64.so.2")
    else:
        glob_patterns.append(sysroot_path + "/lib/ld-linux-{arch}.so.*")

    patterns = [
        pattern.format(arch = sysroot_arch)
        for pattern in glob_patterns
    ]

    exclude = [
        "**/{arch}-*/**"
        for arch in _linux_sysroot_archs
        if arch != sysroot_arch
    ]

    native.filegroup(
        name = name,
        srcs = native.glob(
            patterns,
            exclude = exclude,
            exclude_directories = 1,
            allow_empty = True,
        ),
        visibility = ["//visibility:public"],
    )
