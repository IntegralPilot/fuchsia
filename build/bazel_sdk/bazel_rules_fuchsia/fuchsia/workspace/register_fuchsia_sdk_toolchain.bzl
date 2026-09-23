# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

""" Repository rule for registering the Fuchsia sdk toolchain. """

def _fuchsia_toolchain_decl_impl(ctx):
    ctx.file("WORKSPACE.bazel", content = "", executable = False)
    ctx.file(
        "BUILD.bazel",
        content = """
toolchain(
  name = "{toolchain_name}",
  toolchain = "{toolchain_path}",
  toolchain_type = "@rules_fuchsia//fuchsia/toolchains:sdk",
  visibility = ["//visibility:public"],
)
""".format(toolchain_name = ctx.name + "_toolchain", toolchain_path = ctx.attr.toolchain_path),
        executable = False,
    )

fuchsia_toolchain_decl = repository_rule(
    implementation = _fuchsia_toolchain_decl_impl,
    attrs = {
        "toolchain_path": attr.string(mandatory = True),
    },
)

def register_fuchsia_sdk_toolchain(
        name = "fuchsia_sdk_toolchain_decl",
        toolchain_path = "@fuchsia_sdk//:fuchsia_toolchain_info"):
    """ Registers the fuchsia sdk toolchain.

    This method should be called in your WORKSPACE file to register the fuchsia
    toolchain. It defaults to using the toolchain defined in the fuchsia_sdk
    repository but a different name can be used if needed.

    The toolchain is assumed to exist at the root of the given repo but the path
    can be provided if needed.

    Args:
        name: The toolchain decl repository.
        toolchain_path: The fully qualified path to the toolchain. This should only
          be needed if you are using a non-standard SDK.
    """
    fuchsia_toolchain_decl(
        name = name,
        toolchain_path = toolchain_path,
    )
    native.register_toolchains("@" + name + "//:all")
