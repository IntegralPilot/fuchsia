# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_rules_common//build_flags:cc.bzl",
    "BUILD_FLAGS_CC_ATTRS_KWARGS",
    "wrap_cc_macro_args_with_build_flags",
)
load("@rules_cc//cc:defs.bzl", "cc_library")

def _fx_cc_library_impl(
        name,
        configs,  # buildifier: disable=unused-variable - For GN conversion only.
        public_configs,  # buildifier: disable=unused-variable - For GN conversion only.
        build_flags,
        disable_build_flags,
        **kwargs):
    """Implementation for the fx_cc_library() macro."""

    wrapped_kwargs = wrap_cc_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        cc_rule_name = "cc_library",
        build_flags = build_flags,
        disable_build_flags = disable_build_flags,
        target_type = "cxx_common",
    )

    cc_library(
        name = name,
        **wrapped_kwargs
    )

fx_cc_library = macro(
    doc = """Wrapper for cc_library() libraries for Fuchsia.

    Toolchain overrides can be specified using build_flags and
    disable_build_flags. These will not affect dependencies.
    """,
    implementation = _fx_cc_library_impl,
    # TODO(https://fxbug.dev/446694542): Remove `native.` once the
    # `cc_library()` wrapper is a symbolic macro.
    inherit_attrs = native.cc_library,
    attrs = {
        "configs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
        "public_configs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
    } | BUILD_FLAGS_CC_ATTRS_KWARGS,
)
