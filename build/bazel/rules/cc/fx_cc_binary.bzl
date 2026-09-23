# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load(
    "@fuchsia_rules_common//build_flags:cc.bzl",
    "BUILD_FLAGS_CC_ATTRS_KWARGS",
    "wrap_cc_macro_args_with_build_flags",
)
load("@rules_cc//cc:defs.bzl", "cc_binary")

def _fx_cc_binary_impl(
        name,
        public_configs,  # buildifier: disable=unused-variable - For GN conversion only.
        build_flags,
        disable_build_flags,
        **kwargs):
    """Implementation for the fx_cc_binary() macro."""

    wrapped_kwargs = wrap_cc_macro_args_with_build_flags(
        kwargs = kwargs,
        name = name,
        cc_rule_name = "cc_binary",
        build_flags = build_flags,
        disable_build_flags = disable_build_flags,

        # cc_binary() produces a shared library if linkshared=True, or
        # an executable if it is False (the default).
        target_type = "cxx_shared_library" if kwargs.get("linkshared") else "cxx_executable",
    )

    cc_binary(
        name = name,
        **wrapped_kwargs
    )

fx_cc_binary = macro(
    doc = """Wrapper for cc_binary() binaries for Fuchsia.

    Toolchain overrides can be specified using build_flags and
    disable_build_flags. These will not affect dependencies.
    """,
    implementation = _fx_cc_binary_impl,
    # TODO(https://fxbug.dev/446694542): Remove `native.` once the
    # `cc_binary()` wrapper is a symbolic macro.
    inherit_attrs = native.cc_binary,
    attrs = {
        "public_configs": attr.string_list(
            doc = "Unused in Bazel, for GN conversion only.",
            default = [],
        ),
    } | BUILD_FLAGS_CC_ATTRS_KWARGS,
)
