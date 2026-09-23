# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common utilities for both repository and regular rules."""

# TECHNICAL NOTE
#
# The functions in this file generate labels that assume that the
# top-level `common` directory of the current repository points
# to the shared definitions used by both the Bazel SDK rules
# and the Fuchsia in-tree workspace.
#

_TO_FUCHSIA_OS_MAP = {
    "windows": "win",
}

_TO_FUCHSIA_CPU_MAP = {
    "k8": "x64",
    "x86_64": "x64",
    "aarch64": "arm64",
}

_FUCHSIA_OS_TO_BAZEL_OS = {
    "win": "windows",
}

_TO_BAZEL_CPU_MAP = {
    "arm64": "aarch64",
    "x64": "x86_64",
    "k8": "x86_64",
}

def to_bazel_os_name(target_os):
    """Convert a Bazel or Fuchsia OS name into a canonical Bazel OS name."""
    return _FUCHSIA_OS_TO_BAZEL_OS.get(target_os, target_os)

def to_bazel_cpu_name(target_cpu):
    """Convert Fuchsia or Bazel CPU architecture name to Bazel canonical one.

    Args:
        target_cpu: A Bazel- or Fuchsia-compatible cpu name (e.g. "x64").
    Returns:
        A canonical Bazel compatible cpu name, e.g. "x86_64", so that
        @platforms//cpu:<result> is a valid label.
    """
    return _TO_BAZEL_CPU_MAP.get(target_cpu, target_cpu)

def to_fuchsia_os_name(target_os):
    """Convert Bazel OS name to Fuchsia cpu compatibel one.

    Args:
        target_os: A Fuchsia- or Bazel-compatible operating system name,
    Returns:
        A Fuchsia compatible operating system name string (e.g. "win")
    """
    return _TO_FUCHSIA_OS_MAP.get(target_os, target_os)

def to_fuchsia_cpu_name(target_cpu):
    """Convert Bazel or Fuchsia cpu name to Fuchsia cpu architecture name.

    Args:
        target_cpu: A Fuchsia- or Bazel-compatible cpu name.
    Returns:
        The corresponding Fuchsia cpu name (e.g. 'x64'), same as input
        if this was already a valid Fuchsia name.
    """
    return _TO_FUCHSIA_CPU_MAP.get(target_cpu, target_cpu)

def to_platform_os_constraint(target_os):
    """Return a Bazel platform constraint matching a given target OS.

    Args:
        target_os: Operating system name, using either Fuchsia or Bazel
           conventions.
    Returns:
        A label string (e.g. "@platforms//os:linux").
    """
    return "@platforms//os:" + to_bazel_os_name(target_os)

def to_platform_cpu_constraint(target_arch):
    """Return a Bazel platform constraint matching a given target architecture..

    Args:
        target_arch: CPU architecture name, using either Fuchsia or Bazel
           conventions.
    Returns:
        A label string (e.g. "@platforms//cpu:x86_64").
    """
    return "@platforms//cpu:" + to_bazel_cpu_name(target_arch)

# The list of all supported target tags
all_target_tags = [
    "linux-x64",
    "linux-arm64",
    "fuchsia-x64",
    "fuchsia-arm64",
    "fuchsia-riscv64",
]

def to_target_os_cpu_pair(target_tag):
    """Convert a target tag string to (target_os, target_cpu) pair.

    Args:
        target_tag: a string identifiying a target (os,cpu) pair, using
          either "-" or "_" as the separator.
    Returns:
        A target (os, cpu) string pair.
    """
    target_os, sep, target_cpu = target_tag.replace("_", "-").partition("-")
    if sep != "-":
        fail("Invalid target tag value (<os>-<cpu> expected): " + target_tag)
    return target_os, target_cpu

def config_setting_label_for_target_os_cpu(
        target_os,
        target_cpu,
        common_package_prefix = "//common"):
    """Return label of a config setting holding True for a specific (os, cpu) pair.

    Args:
        target_os: An OS name, using either Bazel or Fuchsia conventions.
        target_cpu: A CPU name, using either Bazel or Fuchsia conventions.
        common_package_prefix: Package prefix for the repository that contains
            this .bzl file and the BUILD.bazel next to it.
    Returns:
        A label string pointing to a config_setting() value which will hold
        true when the current build configuration corresponds to
        (target_os, target_cpu).
    """
    return "%s/platforms:is_%s_%s" % (
        common_package_prefix,
        to_fuchsia_os_name(target_os),
        to_fuchsia_cpu_name(target_cpu),
    )

def config_setting_label_for_target_tag(target_tag, common_package_prefix = "//common"):
    """Return label of a config setting holding True for a specific "os-cpu" pair.

    Args:
        target_tag: a string identifiying a target (os,cpu) pair, using
          either "-" or "_" as the separator.
        common_package_prefix: Package prefix for the repository that contains
            this .bzl file and the BUILD.bazel next to it.
    Returns:
        A label string pointing to a config_setting() value which will hold
        true when the current build configuration corresponds to
        target_os_cpu.
    """
    target_os, target_cpu = to_target_os_cpu_pair(target_tag)
    return config_setting_label_for_target_os_cpu(target_os, target_cpu, common_package_prefix)

def target_tag_dict_to_select_keys(input_dict, add_default = None, common_package_prefix = "//common"):
    """Convert a { target_tag -> value } dictionary into a select() dictionary.

    Args:
        input_dict: A dictionary whose keys are target tag strings,
          and whose values are select()-compatible values (e.g. label lists
          or glob() call expressions).
        add_default: If not None, add a //conditions:default clause matching
          this value.
        common_package_prefix: Package prefix for the repository that contains
            this .bzl file and the BUILD.bazel next to it.
    Returns:
        A dictionary whose keys are config setting labels matching the
        target_tag keys from the input, associated with the corresponding
        input values.

    Example:
       For the following input dictionary:

          {
            "linux-x64": [ "//package;name1" ],
            "fuchsia-aarch64": [ "//package:name2" ],
          }

       target_tag_dict_to_select_keys(input_dict, [], "@repo//common") returns

          {
            "@repo//common/platforms:is_linux_x64": [
                "//package:name1"
            ],
            "@repo//common/platforms:is_fuchsia_arm64": [
                "//package:name2",
            ],
            "//conditions:default": [],
          }
    """
    result_dict = {
        config_setting_label_for_target_tag(target_tag, common_package_prefix): value
        for target_tag, value in input_dict.items()
    }
    if add_default != None:
        result_dict["//conditions:default"] = add_default

    return result_dict
