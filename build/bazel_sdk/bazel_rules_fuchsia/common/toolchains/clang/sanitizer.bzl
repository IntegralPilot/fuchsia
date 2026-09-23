# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Sanitizers definitions for Clang.
"""

load("@bazel_skylib//lib:selects.bzl", "selects")
load("@rules_cc//cc:action_names.bzl", "ALL_CC_COMPILE_ACTION_NAMES", "ALL_CC_LINK_ACTION_NAMES")
load(
    "@rules_cc//cc:cc_toolchain_config_lib.bzl",
    "feature",
    "flag_group",
    "flag_set",
    "with_feature_set",
)
load(
    "//common:toolchains/clang/feature_flag.bzl",
    "feature_flag",
)

# This feature should only be enabled by one of the features below
# and will drop the optimization level and raise the debug info detail.
_clang_sanitizer_feature = feature(
    name = "clang_sanitizer",
    flag_sets = [
        flag_set(
            actions = ALL_CC_COMPILE_ACTION_NAMES,
            flag_groups = [
                flag_group(
                    flags = [
                        "-fno-omit-frame-pointer",
                        "-g3",
                        "-O1",
                    ],
                ),
            ],
        ),
    ],
)

def _sanitizer_mode_feature(name, mode):
    """Define a feature that corresponds to Clang -fsanitize=<mode>.

    The flag will be added both to compile and link actions, and all
    instances returned by this function will be mutually exclusive,
    to reflect Clang's own constraints.

    For example, when using '-fsanitize=address -fsanitize=hwaddress'
    Clang will complain with an error stating that this is not allowed.
    """
    return feature(
        name = name,
        flag_sets = [
            flag_set(
                actions = ALL_CC_COMPILE_ACTION_NAMES + ALL_CC_LINK_ACTION_NAMES,
                flag_groups = [flag_group(flags = ["-fsanitize=" + mode])],
            ),
        ],
        implies = ["clang_sanitizer"],

        # This ensures mutual exclusion between the different values returned
        # by this function. Bazel will print an error message. For example when
        # both `--features=asan` and `--features=hwasan` are used:
        #
        # ```
        # Analyzing: target //build/bazel/rules/host_tests/tests/cc:static_test (58 packages loaded, 9 targets configured)
        # ERROR: ...../out/default/gen/build/bazel/workspace/build/bazel/rules/host_tests/tests/cc/BUILD.bazel:12:11: in cc_library rule //build/bazel/rules/host_tests/tests/cc:foo:
        # Traceback (most recent call last):
        #         File "/virtual_builtins_bzl/common/cc/cc_library.bzl", line 33, column 57, in _cc_library_impl
        #         File "/virtual_builtins_bzl/common/cc/cc_common.bzl", line 184, column 49, in _configure_features
        # Error in configure_features: Symbol clang_sanitizer_mode is provided by all of the following features: asan hwasan
        # ```
        provides = ["clang_sanitizer_mode"],
    )

# All these features correspond to mutually exclusive Clang sanitizer modes.
_asan_feature = _sanitizer_mode_feature("asan", "address")
_hwasan_feature = _sanitizer_mode_feature("hwasan", "hwaddress")
_msan_feature = _sanitizer_mode_feature("msan", "memory")
_tsan_feature = _sanitizer_mode_feature("tsan", "thread")

# The lsan feature corresponds to -fsanitize=leak which is compatible
# with either -fsanitize=address or -fsanitize=hwaddress, because their
# respective runtime (e.g. libclang_rt.asan.so) does implement the
# required support.
#
# To support this, only add the linker flag when neither asan or hwasan
# are enabled.
_lsan_feature = feature(
    name = "lsan",
    flag_sets = [
        flag_set(
            actions = ALL_CC_COMPILE_ACTION_NAMES,
            flag_groups = [flag_group(flags = ["-fsanitize=leak"])],
        ),
        flag_set(
            actions = ALL_CC_LINK_ACTION_NAMES,
            flag_groups = [flag_group(flags = ["-fsanitize=leak"])],
            with_features = [
                with_feature_set(
                    not_features = ["asan", "hwasan"],
                ),
            ],
        ),
    ],
    implies = ["clang_sanitizer"],
)

# The ubsan feature corresponds to -fsanitize=undefined at compile time
# and is also compatible with -fsanitize={address,hwaddress} for the same
# reasons as lsan, so implement a similar scheme.
_ubsan_feature = feature(
    name = "ubsan",
    flag_sets = [
        flag_set(
            actions = ALL_CC_COMPILE_ACTION_NAMES,
            flag_groups = [flag_group(flags = ["-fsanitize=undefined"])],
        ),
        flag_set(
            actions = ALL_CC_LINK_ACTION_NAMES,
            flag_groups = [flag_group(flags = ["-fsanitize=undefined"])],
            with_features = [
                with_feature_set(
                    not_features = ["asan", "hwasan"],
                ),
            ],
        ),
    ],
    implies = ["clang_sanitizer"],
)

sanitizer_features = [
    _clang_sanitizer_feature,
    _asan_feature,
    _hwasan_feature,
    _msan_feature,
    _tsan_feature,
    _lsan_feature,
    _ubsan_feature,
]

def define_clang_sanitizer_config_settings():
    """Create sanitizer-related feature_flag() and config_setting() targets.

    This function must be called from the BUILD.bazel of a given
    Clang external repository, it defines multiple config_setting()
    targets that other rules or functions depend on (e.g. package_resources.bzl)
    """

    # Holds True if --features=asan is used in the current build configuration
    # or through a target-specific `features = [...]` definition.
    feature_flag(
        name = "asan_feature_flag",
        feature_name = "asan",
        visibility = ["//visibility:private"],
    )

    # Holds True if --features=hwasan is used in the current build configuration
    # or through a target-specific `features = [...]` definition.
    feature_flag(
        name = "hwasan_feature_flag",
        feature_name = "hwasan",
        visibility = ["//visibility:private"],
    )

    # The following config_setting() determine which sanitizer mode
    # is enabled.
    #
    # IMPORTANT: The hwasan feature takes precedence over the asan one.
    # Keep this in sync with the definition of sanitizer_features in
    # //common:toolchains/clang/sanitizer.bzl

    # Holds True if no Asan features are enabled.
    native.config_setting(
        name = "novariant",
        flag_values = {
            ":asan_feature_flag": "False",
            ":hwasan_feature_flag": "False",
        },
        visibility = ["//visibility:public"],
    )

    # Holds True if 'asan' feature is enabled, but 'hwasan' is not.
    native.config_setting(
        name = "asan_variant",
        flag_values = {
            ":asan_feature_flag": "True",
            ":hwasan_feature_flag": "False",
        },
        visibility = ["//visibility:public"],
    )

    # Holds True if 'hwasan' feature is enabled.
    native.config_setting(
        name = "hwasan_variant",
        flag_values = {
            ":hwasan_feature_flag": "True",
            # ignore asan_feature_flag intentionally.
        },
        visibility = ["//visibility:public"],
    )

    # Cpu specific config_filters for novariant/asan/hwasan.
    native.config_setting(
        name = "arm64_build",
        constraint_values = ["@platforms//cpu:aarch64"],
    )
    native.config_setting(
        name = "x64_build",
        constraint_values = ["@platforms//cpu:x86_64"],
    )
    native.config_setting(
        name = "riscv64_build",
        constraint_values = ["@platforms//cpu:riscv64"],
    )
    #
    # Note that since that the SDK's fuchsia_transition() function
    # always changes --platforms, checking against @platforms//cpu:<name>
    # is enough here.
    #
    # The SDK Bazel rules used to also check against the command-line
    # --cpu value (also changed by the transition function), which
    # is not needed anymore since the introduction of --platforms
    # in Bazel 7.

    selects.config_setting_group(
        name = "arm64_novariant",
        match_all = [
            ":arm64_build",
            ":novariant",
        ],
    )

    selects.config_setting_group(
        name = "arm64_asan_variant",
        match_all = [
            ":arm64_build",
            ":asan_variant",
        ],
    )

    selects.config_setting_group(
        name = "arm64_hwasan_variant",
        match_all = [
            ":arm64_build",
            ":hwasan_variant",
        ],
    )

    selects.config_setting_group(
        name = "x64_novariant",
        match_all = [
            ":x64_build",
            ":novariant",
        ],
    )

    selects.config_setting_group(
        name = "x64_asan_variant",
        match_all = [
            ":x64_build",
            ":asan_variant",
        ],
    )

    selects.config_setting_group(
        name = "x64_hwasan_variant",
        match_all = [
            ":x64_build",
            ":hwasan_variant",
        ],
    )

    selects.config_setting_group(
        name = "riscv64_novariant",
        match_all = [
            ":riscv64_build",
            ":novariant",
        ],
    )

    selects.config_setting_group(
        name = "riscv64_asan_variant",
        match_all = [
            ":riscv64_build",
            ":asan_variant",
        ],
    )

    selects.config_setting_group(
        name = "riscv64_hwasan_variant",
        match_all = [
            ":riscv64_build",
            ":hwasan_variant",
        ],
    )
