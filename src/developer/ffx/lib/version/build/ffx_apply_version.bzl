# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines the ffx_apply_version rule for applying version info to ffx binaries.

This rule solves the problem that ffx, being the largest binary in the
build, takes a long time to link. It is desirable to minimize causes of cache
invalidations for builds of this binary, so as to save build time. One of the
causes of invalidations is changes in repository version information, that
will occur for every patch in CI/CQ, even if no source files for ffx or its
dependencies change. Applying the version post-link avoids these costs, as it
is cheap compared to re-linking the binary.

This rule works with both stripped and unstripped binaries as input, but will
always produce a stripped binary. The build-id directory entry for the stripped
binary is not produced, if that's needed, it should be produced in a separate
action, and this rule used with the resultant stripped binary.
"""

load("@rules_cc//cc:find_cc_toolchain.bzl", "CC_TOOLCHAIN_ATTRS", "find_cc_toolchain", "use_cc_toolchain")

def _ffx_apply_version_impl(ctx):
    cc_toolchain = find_cc_toolchain(ctx)
    unversioned = ctx.file.unversioned_binary
    versioned = ctx.actions.declare_file(ctx.attr.name)

    args = ctx.actions.args()

    # -x applies a "strip" on the output. Strip is currently performed after the link
    # step in the normal toolchain rules, and uses this same flag on most platforms.
    args.add("-x")
    args.add("--update-section=.ffx_version=%s" % ctx.file._version_info.path)
    args.add("--update-section=.ffx_build=%s" % ctx.file._build_version.path)
    args.add(unversioned)
    args.add(versioned)

    ctx.actions.run(
        executable = cc_toolchain.objcopy_executable,
        arguments = [args],
        inputs = [unversioned, ctx.file._version_info, ctx.file._build_version],
        outputs = [versioned],
        tools = cc_toolchain.all_files,
        mnemonic = "FfxApplyVersion",
        progress_message = "Applying version to %s" % ctx.label,
    )

    return [
        DefaultInfo(files = depset([versioned]), executable = versioned),
    ]

ffx_apply_version = rule(
    implementation = _ffx_apply_version_impl,
    toolchains = use_cc_toolchain(),
    attrs = {
        "unversioned_binary": attr.label(
            doc = "Unversioned binary",
            mandatory = True,
            allow_single_file = True,
        ),
        "_version_info": attr.label(
            doc = "Version info binary file",
            default = "//src/developer/ffx/lib/version/build:version_info.bin",
            allow_single_file = True,
        ),
        "_build_version": attr.label(
            doc = "Build version binary file",
            default = "//src/developer/ffx/lib/version/build:build_version.bin",
            allow_single_file = True,
        ),
    } | CC_TOOLCHAIN_ATTRS,  # Needed by find_cc_toolchain
    executable = True,
)
