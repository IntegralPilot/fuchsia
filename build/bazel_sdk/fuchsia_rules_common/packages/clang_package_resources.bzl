# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Clang runtime libraries as Fuchsia package resources.

This provides the get_clang_package_resources() function which
returns a struct describing how and when various Clang runtime
shared libraries (e.g. for libc++ and sanitizer runtimes) should be
installed into Fuchsia packages.

The struct provides various values that can be used to define targets
using the SDK's `fuchsia_package_resource_group()` rule, or using a
similar custom in-tree rule for the "fuchsia_platform" platform.

See documentation comments in the function at the end of this file for
details and an example.
"""

# The following dictionary is used to describe the libc++ shared libraries
# that must be added to Fuchsia package that contain binaries linked against
# libc++. There is a similar one for sanitizer runtimes below.
#
# They both share the same schema where keys are cpu names, following fuchsia
# conventions. Values are dictionaries that map the label of a Bazel
# config_setting() to a list of label expressions to runtime files provided by
# the Clang repository.
#
# The label expressions use the following template parameters:
#
#  - {clang_repo}: The Clang repository prefix (e.g. "@fuchsia_clang//")
#
#  - {clang_lib_dir}: The Clang internal library directory (e.g.
#    lib/clang/<version>/lib) relative to the clang installation directory.
#
# IMPORTANT: The keys in these maps depend on the config_setting() targets
# defined through the define_clang_sanitizer_config_settings() function!
#
_LIBCXX_SRCS_MAP = {
    "arm64": {
        "{clang_repo}:arm64_novariant": [
            "{clang_repo}:lib/aarch64-unknown-fuchsia/libc++.so.2",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/libc++abi.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/libunwind.so.1",
        ],
        "{clang_repo}:arm64_asan_variant": [
            "{clang_repo}:lib/aarch64-unknown-fuchsia/asan/libc++.so.2",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/asan/libc++abi.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/asan/libunwind.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/asan+noexcept/libc++.so.2",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/asan+noexcept/libc++abi.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/asan+noexcept/libunwind.so.1",
        ],
        "{clang_repo}:arm64_hwasan_variant": [
            "{clang_repo}:lib/aarch64-unknown-fuchsia/hwasan/libc++.so.2",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/hwasan/libc++abi.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/hwasan/libunwind.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/hwasan+noexcept/libc++.so.2",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/hwasan+noexcept/libc++abi.so.1",
            "{clang_repo}:lib/aarch64-unknown-fuchsia/hwasan+noexcept/libunwind.so.1",
        ],
        "//conditions:default": [],
    },
    "riscv64": {
        "{clang_repo}:riscv64_novariant": [
            "{clang_repo}:lib/riscv64-unknown-fuchsia/libc++.so.2",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/libc++abi.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/libunwind.so.1",
        ],
        "{clang_repo}:riscv64_asan_variant": [
            "{clang_repo}:lib/riscv64-unknown-fuchsia/asan/libc++.so.2",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/asan/libc++abi.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/asan/libunwind.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/asan+noexcept/libc++.so.2",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/asan+noexcept/libc++abi.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/asan+noexcept/libunwind.so.1",
        ],
        "{clang_repo}:riscv64_hwasan_variant": [
            "{clang_repo}:lib/riscv64-unknown-fuchsia/hwasan/libc++.so.2",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/hwasan/libc++abi.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/hwasan/libunwind.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/hwasan+noexcept/libc++.so.2",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/hwasan+noexcept/libc++abi.so.1",
            "{clang_repo}:lib/riscv64-unknown-fuchsia/hwasan+noexcept/libunwind.so.1",
        ],
        "//conditions:default": [],
    },
    "x64": {
        "{clang_repo}:x64_novariant": [
            "{clang_repo}:lib/x86_64-unknown-fuchsia/libc++.so.2",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/libc++abi.so.1",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/libunwind.so.1",
        ],
        "{clang_repo}:x64_asan_variant": [
            "{clang_repo}:lib/x86_64-unknown-fuchsia/asan/libc++.so.2",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/asan/libc++abi.so.1",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/asan/libunwind.so.1",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/asan+noexcept/libc++.so.2",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/asan+noexcept/libc++abi.so.1",
            "{clang_repo}:lib/x86_64-unknown-fuchsia/asan+noexcept/libunwind.so.1",
        ],
        # NOTE: hwasan is not supported on x64
        "//conditions:default": [],
    },
}

# The following dictionary is used to describe the installation directory
# of libc++ shared libraries. To be used, after applying the repository prefix,
# in a select() statement for the "dest" field of fuchsia_package_resource_group().
_LIBCXX_DEST_SELECT_MAP = {
    "{clang_repo}:asan_variant": "lib/asan",
    "{clang_repo}:hwasan_variant": "lib/hwasan",
    "//conditions:default": "lib",
}

# The following dictionary is used to describe the "strip_prefix" argument of a
# fuchsia_package_resource_group(). In practice, a repository prefix must be
# applied, then the value passed to _filter_cpu_config_label_map to remove
# entries that are not related to a supported CPU architecture.
_LIBCXX_STRIP_PREFIX_MAP = {
    "arm64": {
        "{clang_repo}:arm64_novariant": "lib/aarch64-unknown-fuchsia",
        "{clang_repo}:arm64_asan_variant": "lib/aarch64-unknown-fuchsia/asan",
        "{clang_repo}:arm64_hwasan_variant": "lib/aarch64-unknown-fuchsia/hwasan",
    },
    "riscv64": {
        "{clang_repo}:riscv64_novariant": "lib/riscv64-unknown-fuchsia",
        "{clang_repo}:riscv64_asan_variant": "lib/riscv64-unknown-fuchsia/asan",
        "{clang_repo}:riscv64_hwasan_variant": "lib/riscv64-unknown-fuchsia/hwasan",
    },
    "x64": {
        "{clang_repo}:x64_novariant": "lib/x86_64-unknown-fuchsia",
        "{clang_repo}:x64_asan_variant": "lib/x86_64-unknown-fuchsia/asan",
        "{clang_repo}:x64_hwasan_variant": "",  # hwasan not supported on x64.
    },
}

# The three dictionaries below correspond to the sanitizer runtimes.

_SANITIZERS_SRCS_MAP = {
    "arm64": {
        "{clang_repo}:arm64_asan_variant": [
            "{clang_repo}:{clang_lib_dir}/aarch64-unknown-fuchsia/libclang_rt.asan.so",
        ],
        "{clang_repo}:arm64_hwasan_variant": [
            "{clang_repo}:{clang_lib_dir}/aarch64-unknown-fuchsia/libclang_rt.hwasan.so",
        ],
        "//conditions:default": [],
    },
    "riscv64": {
        "{clang_repo}:riscv64_asan_variant": [
            "{clang_repo}:{clang_lib_dir}/riscv64-unknown-fuchsia/libclang_rt.asan.so",
        ],
        "{clang_repo}:riscv64_hwasan_variant": [
            "{clang_repo}:{clang_lib_dir}/riscv64-unknown-fuchsia/libclang_rt.hwasan.so",
        ],
        "//conditions:default": [],
    },
    "x64": {
        "{clang_repo}:x64_asan_variant": [
            "{clang_repo}:{clang_lib_dir}/x86_64-unknown-fuchsia/libclang_rt.asan.so",
        ],
        # NOTE: hwasan is not supported on x64.
        "//conditions:default": [],
    },
}

_SANITIZERS_DEST_SELECT_MAP = {
    "{clang_repo}:asan_variant": "lib/asan",
    "{clang_repo}:hwasan_variant": "lib/hwasan",
    "//conditions:default": "lib",
}

_SANITIZERS_STRIP_PREFIX_MAP = {
    "arm64": {
        "{clang_repo}:arm64_build": "{clang_lib_dir}/aarch64-unknown-fuchsia",
    },
    "riscv64": {
        "{clang_repo}:riscv64_build": "{clang_lib_dir}/riscv64-unknown-fuchsia",
    },
    "x64": {
        "{clang_repo}:x64_build": "{clang_lib_dir}/x86_64-unknown-fuchsia",
    },
}

def _filter_cpu_config_label_map(d, supported_cpus):
    """Filter a { cpu -> { config_label -> value } } map according to a list of valid cpus.

    This is similar to the SDK's fuchsia_cpu_select() function, without
    depending on @rules_fuchsia.

    Args:
        d: A dictionary implementing a `{ cpu -> { config_label -> value } }` map,
           where each `config_label` points to a CPU-specific `config_setting()` target.
           Each `config_label` must be unique across all `cpu` dictionaries.

        supported_cpus: a list or set of cpu names, using Fuchsia conventions.
    Returns:
        A new `{ config_label -> value }` dictionary, where all the `config_label`s
        correspond to cpus from supported_cpus and the input map.
    """
    result = {}
    for cpu in supported_cpus:
        cpu_dict = d.get(cpu, {})
        for config_label, value in cpu_dict.items():
            current = result.setdefault(config_label, value)
            if current != value:
                fail("Cannot use config_setting() label {} twice in input to _filter_cpu_config_label_map()".format(config_label))
    return result

def _filter_and_format_srcs(srcs_map, supported_cpus, **format_kwargs):
    """Filter a sources map by cpu, then format its content."""
    srcs = _filter_cpu_config_label_map(srcs_map, supported_cpus)
    return {
        condition.format(**format_kwargs): [label.format(**format_kwargs) for label in labels]
        for condition, labels in srcs.items()
    }

def _format_dest_selects(dest_select_map, **format_kwargs):
    """Format the content of a dest select map."""
    return {
        condition.format(**format_kwargs): dest_path
        for condition, dest_path in dest_select_map.items()
    }

def _filter_and_format_strip_prefixes(strip_prefixes_map, supported_cpus, **format_kwargs):
    """Filter a strip prefixes map by cpu, then format its content."""
    strip_prefixes = _filter_cpu_config_label_map(strip_prefixes_map, supported_cpus)
    return {
        condition.format(**format_kwargs): prefix.format(**format_kwargs)
        for condition, prefix in strip_prefixes.items()
    }

def get_clang_package_resources(clang_repo_prefix, lib_clang_internal_dir, supported_cpus):
    """Get information about Clang-related Fuchsia package resources.

    This function returns a struct that provides various maps that can be used
    to call fuchsia_package_resource_group() or a similar in-tree custom rule to
    record which Clang-related runtime library files should be installed into Fuchsia
    packages under various conditions.

    This is useful to avoid depending on @fuchsia_sdk within the @fuchsia_clang
    repository. For details see https://fxbug.dev/514679143.

    Args:
        clang_repo_prefix: Either an empty string or a Clang repository prefix
            such as "@fuchsia_clang//".
        lib_clang_internal_dir: The Clang internal directory, relative to
            the clang repo prefix, e.g. "lib/clang/<version>".
        supported_cpus: A list of supported cpu names, using Fuchsia conventions.

    Returns:
        A struct that contains various constant dictionaries that can be used
        to call the fuchsia_package_resource_group() rule, or an equivalent in-tree
        custom platform rule to do the same.

        The following three fields are related to the C++ runtime libraries:

        libcxx_srcs_select_map
            A `{ config_label -> [ file_label ] }` dictionary that can be passed
            to `select()` to compute the value of the `srcs` attribute.

        libcxx_dest_select_map
            A `{ config_label -> install_path }` dictionary, that can be used
            in a `select()` statement for the `dest` attribute, describing where
            the sources from `libcxx_srcs_select_map` should be installed into the
            Fuchsia package.

        libcxx_strip_prefix_select_map
            A `{ config_label : prefix_string }` dictionary. Can be passed to
            `select()` to compute the value of the `strip_prefix` attribute.

        A typical use case would look like this:

        ```
        load("@fuchsia_clang//:generated_constants.bzl", clang_constants = "constants")

        load(
            "@fuchsia_rules_common//packages:clang_package_resources.bzl",
            "get_clang_package_resources",
        )
        load(
            "@fuchsia_rules_common//packages:resources.bzl",
            "fuchsia_package_resource_group",
        )

        SUPPORTED_SDK_CPUS = [ ... ]

        clang_package_resources = get_clang_package_resources(   # <---- CALL THIS FUNCTION HERE
            clang_repo_prefix = "@fuchsia_clang//",
            lib_clang_internal_dir = clang_constants.lib_clang_internal_dir,
            supported_cpus = SUPPORTED_SDK_CPUS,
        )

        fuchsia_package_resource_group(
            name = "libcxx.dist",
            srcs = select(clang_package_resources.libcxx_srcs_select_map),
            dest = select(clang_package_resources.libcxx_dest_select_map),
            strip_prefix = select(clang_package_resources.libcxx_strip_prefix_select_map),
            ...
        )
        ```

        The following three fields are related to sanitizer runtimes, their purpose being
        similar to the libcxx_ ones above.

        sanitizers_srcs_select_map
        sanitizers_dest_select_map
        sanitizers_strip_prefix_select_map
    """
    format_args = {
        "clang_repo": clang_repo_prefix,
        "clang_lib_dir": "{}/lib".format(lib_clang_internal_dir),
    }
    return struct(
        libcxx_srcs_select_map = _filter_and_format_srcs(_LIBCXX_SRCS_MAP, supported_cpus, **format_args),
        libcxx_dest_select_map = _format_dest_selects(_LIBCXX_DEST_SELECT_MAP, **format_args),
        libcxx_strip_prefix_select_map = _filter_and_format_strip_prefixes(_LIBCXX_STRIP_PREFIX_MAP, supported_cpus, **format_args),
        sanitizers_srcs_select_map = _filter_and_format_srcs(_SANITIZERS_SRCS_MAP, supported_cpus, **format_args),
        sanitizers_dest_select_map = _format_dest_selects(_SANITIZERS_DEST_SELECT_MAP, **format_args),
        sanitizers_strip_prefix_select_map = _filter_and_format_strip_prefixes(_SANITIZERS_STRIP_PREFIX_MAP, supported_cpus, **format_args),
    )
