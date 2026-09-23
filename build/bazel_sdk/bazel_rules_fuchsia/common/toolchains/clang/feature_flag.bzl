# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Definition of the feature_flag() rule."""

load(
    "@bazel_tools//tools/cpp:toolchain_utils.bzl",
    "find_cpp_toolchain",
    "use_cpp_toolchain",
)
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")

def _feature_flag(ctx):
    toolchain = find_cpp_toolchain(ctx)
    feature = ctx.attr.feature_name
    feature_configuration = cc_common.configure_features(
        ctx = ctx,
        cc_toolchain = toolchain,
        requested_features = ctx.features,
        unsupported_features = ctx.disabled_features,
    )
    return [config_common.FeatureFlagInfo(value = str(cc_common.is_enabled(
        feature_configuration = feature_configuration,
        feature_name = feature,
    )))]

feature_flag = rule(
    implementation = _feature_flag,
    doc = "A flag whose value corresponds to whether a toolchain feature is enabled. " +
          "For instance if `feature_name == \"asan\"` and `--features=asan` " +
          "is passed on the command-line, the value will be True.",
    attrs = {
        "feature_name": attr.string(),
    },
    fragments = ["cpp"],
    toolchains = use_cpp_toolchain(),
)
