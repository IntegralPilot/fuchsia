# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utilities related to Clang."""

load(
    "//common/platforms:utils.bzl",
    "all_target_tags",
    "target_tag_dict_to_select_keys",
    "to_bazel_cpu_name",
    "to_bazel_os_name",
    "to_fuchsia_cpu_name",
    "to_fuchsia_os_name",
    "to_target_os_cpu_pair",
)

def process_clang_builtins_output(probe_output):
    """Get Clang builtin data

    Args:
      probe_output: The stderr output of running `clang -x c++ -E -v ./empty`
    Returns:
      A tuple of:
        - The clang short version, as a string.
        - The clang long version, as a string.
        - A list of builtin include paths, as a list of path strings.
    """
    short_version = None
    long_version = None
    builtin_include_paths = []

    has_include_paths = False
    has_version = False
    clang_version_prefix = "clang version "
    for line in probe_output.splitlines():
        if not has_version:
            # Example inputs:
            # Fuchsia clang version 15.0.0 (https://llvm.googlesource.com/a/llvm-project 3a20597776a5d2920e511d81653b4d2b6ca0c855)
            # Debian clang version 14.0.6-2
            pos = line.find(clang_version_prefix)
            if pos >= 0:
                long_version = line[pos + len(clang_version_prefix):].strip()

                # Remove space followed by opening parenthesis.
                pos = long_version.find("(")
                if pos >= 0:
                    long_version = long_version[:pos].rstrip()

                # Remove -suffix
                pos = long_version.find("-")
                if pos >= 0:
                    long_version = long_version[:pos]

                # Split at dots to get the short version.
                short_version, _, _ = long_version.partition(".")
                has_version = True
        if not has_include_paths:
            if line == "#include <...> search starts here:":
                has_include_paths = True
        elif line.startswith(" /"):
            if line.startswith((" /usr/include", " /usr/local/include")):
                # ignore lines like /usr/include which should not be used by
                # our build system. Note that some users have their home
                # directory under /usr/local/something/.... and such paths
                # should not be filtered out.
                # See https://fxbug.dev/42062023 for details.
                continue
            builtin_include_paths.append(line.strip())

    return (short_version, long_version, builtin_include_paths)

_TARGET_TRIPLE_MAP = {
    "fuchsia-x64": "x86_64-unknown-fuchsia",
    "fuchsia-arm64": "aarch64-unknown-fuchsia",
    "fuchsia-riscv64": "riscv64-unknown-fuchsia",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "linux-riscv64": "riscv64-unknown-linux-gnu",
}

def to_clang_target_tuple(target_os, target_arch):
    """Return the Clang/GCC target triple for a given (os,arch) pair.

    Args:
      target_os: Target os name, following Fuchsia or Bazel conventions.
      target_arch: Target cpu name, following Fuchsia or Bazel conventions.
    Returns:
      Clang target tuple string (e.g. "x86_64-unknown-linux-gnu")
    """
    target_key = "%s-%s" % (
        to_fuchsia_os_name(target_os),
        to_fuchsia_cpu_name(target_arch),
    )
    triple = _TARGET_TRIPLE_MAP.get(target_key)
    if not triple:
        fail("Unknown os/arch combo: %s, %s" % (target_os, target_arch))
    return triple

# Used internally by format_target_tag_labels_dict() below.
def _get_target_tag_template_dict(target_tag):
    target_os, target_cpu = to_target_os_cpu_pair(target_tag)
    return {
        "os": to_fuchsia_os_name(target_os),
        "cpu": to_fuchsia_cpu_name(target_cpu),
        "bazel_os": to_bazel_os_name(target_os),
        "bazel_cpu": to_bazel_cpu_name(target_cpu),
        "clang_target_tuple": to_clang_target_tuple(target_os, target_cpu),
    }

def format_target_tag_labels_dict(input_dict, extra_dict = None):
    """Format the labels in a { target_tag -> label_list } dictionary.

    Replace each label in the dictionary's values by a formatted string,
    recognizing the following hard-coded substitutions:

      {os} -> Fuchsia OS name matching the target_tag
      {cpu} -> Fuchsia CPU name matching the target_tag
      {bazel_os} -> Bazel OS name
      {bazel_cpu} -> Bazel CPU name
      {clang_target_tuple} -> Clang target tuple for the target_tag.

    If it possible to pass additional formatting arguments using
    the extra_dict argument.

    Args:
        input_dict: A dictionary whose keys are target tag strings,
          and whose values are list of label strings.
        extra_dict: An optional dictionary containing additional
          arguments for the string formatting.

    Return:
        A new { target_tag -> label_list } dictionary.

    Example:
       For the following input dictionary:

          {
            "linux-x64": [ "//{dir}:name_{os}_{cpu}" ],
            "fuchsia-aarch64": [ "//{dir}:name_{os}_{cpu}" ],
          }

       format_target_tag_labels_dict(input_dict, { "dir": "package" })
       will return:

          {
            "linux-x64": [ "//package:name_linux_x64" ],
            "fuchsia-aarch64": [ "//package:name_fuchsia_arm64" ],
          }
    """
    extra_dict = extra_dict or {}
    return {
        target_tag: [
            label.format(**(extra_dict | _get_target_tag_template_dict(target_tag)))
            for label in labels
        ]
        for target_tag, labels in input_dict.items()
    }

def format_labels_list_to_target_tag_dict(
        labels,
        target_tags = None,
        extra_dict = None):
    """Format a list of labels into a { target_tag -> label_list } dictionary.

    The dictionary will have target_tag keys, associated with expanded strings
    matching the tag.

    Args:
        labels: A list of label strings which can contain substitution
           expressions that will be expanded by this function.
        target_tags: An optional list of target tags to populate the
           result dictionary's keys. The default is all supported
           target tags will be used.
        extra_dict: An optional dictionary containing additional arguments
           for the string expansion.

    Example:

        format_labels_list_to_target_tag_dict([
            "//{pkg}:name_{os}_{cpu}",
        ], extra_dict = { "pkg": "package" })

    Returns

        {
          "fuchsia-x64": [ "//package:name_fuchsia_x64" ],
          "fuchsia-arm64": [ "//package:name_fuchsia_arm64" ],
          "fuchsia-riscv64": [ "//package:name_fuchsia_riscv64" ],
          "linux-x64": [ "package:name_linux_x64" ],
          "linux-arm64":  [ "package:name_linux_arm64" ]
        }
    """
    target_tags = target_tags or all_target_tags
    extra_dict = extra_dict or {}
    return {
        target_tag: [
            label.format(**(extra_dict | _get_target_tag_template_dict(target_tag)))
            for label in labels
        ]
        for target_tag in target_tags
    }

def format_labels_list_to_target_tag_native_glob_select(labels, target_tags = None, extra_dict = None, allow_empty = False, common_package_prefix = "//common"):
    """Format a list of label patterns into a select() statement.

    The result is a select() statement whose keys are config_setting() labels
    matching the target_tags, and whose values are native.glob() calls
    taking lists of expanded label patterns as input.

    Args:
        labels: A list of label strings which can contain substitution
           expressions that will be expanded by this function.
        target_tags: An optional list of target tags to populate the
           result dictionary's keys. If None (the default), all supported
           target tags will be used.
        extra_dict: An optional dictionary containing additional arguments
           for the string expansion.
        allow_empty: Optional boolean for glob() calls. Default is False.
        common_package_prefix: Package prefix for the repository that contains
            this .bzl file and the BUILD.bazel next to it.

    Example:

        format_labels_list_to_target_native_glob_select([
            "//{dir}{clang_target_tuple}/**",
        ], extra_dict = { "dir": "package:" })

    Returns

        select({
            "//common/platforms:is_fuchsia_x64": native.glob([
                "//package:x86_64-unknown-fuchsia/**",
            ]),
            "//common/platforms:is_fuchsia_arm64": native.glob([
                "//package:aarch64-unknown-fuchsia/**",
            ]),
            "//common/platforms:is_fuchsia_riscv64": native.glob([
                "//package:riscv64-unknown-fuchsia/**",
            ]),
            "//common/platforms:is_linux_x64": native.glob([
                "//package:x86_64-unknown-linux-gnu/**",
            ]),
            "//common/platforms:is_linux_arm64": native.glob([
                "//package:aarch64-unknown-linux-gnu/**",
            ]),
            "//conditions:default": [],
        })
    """
    target_tags = target_tags or all_target_tags
    extra_dict = extra_dict or {}

    return select(target_tag_dict_to_select_keys(
        {
            target_tag: native.glob(target_labels, allow_empty = allow_empty)
            for target_tag, target_labels in format_labels_list_to_target_tag_dict(
                labels,
                target_tags = target_tags,
                extra_dict = extra_dict,
            ).items()
        },
        add_default = [],
        common_package_prefix = common_package_prefix,
    ))

clang_all_target_tags = all_target_tags
