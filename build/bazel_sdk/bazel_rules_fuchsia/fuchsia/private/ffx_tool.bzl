# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common definition for rules using the ffx tool."""

# Many ffx tools have implicit runtime requirements on other tools that
# are not properly listed in the IDK metadata, and thus must be hard-coded
# here.
#
# Moreover, ffx parses at runtime the IDK metadata files to extract the
# location of said host tools. However, when used in the in-tree Bazel
# workspace, these extracted paths are incorrect: they are Bazel target
# labels (e.g. `@fuchsia_idk//tools/x64:cmc`) that `ffx` cannot resolve
# to the correct Bazel output_base artifact location (which itself cannot
# be predicted when the IDK metadata files are generated).
#
# This can however be worked-around by passing the correct Bazel output path
# for said tools with an explicit command-line overrides (e.g.
# `sdk.overrides.<tool>=<path>`) when invoking the tool.
#
# For each supported tool, two functions are provided here:
#
# - `get_ffx_<tool>_inputs()`: Return a list of File values to expose runtime
#   tool dependencies to the command's sandbox.
#
# - `get_ffx_<tool>_args()`: Return the command prefix that contains the
#   right set of configuration overrides to access the host tool binaries.
#
# Both take a `FuchsiaToolchainInfo` value as input.
#
# Technically, the `sdk.overrides.xxx` configuration options are not required
# when this file is used in OOT Bazel workspaces, but using them
# unconditionally keeps everything simpler.
#
# Note that the SDK manifest must still be exposed to the sandbox, as ffx will
# still read it even if all the paths it needs are overridden.
#
# TODO(https://fxbug.dev/287355615): Expose the runtime dependencies in the
# IDK metadata for each tool. This would allow the computations performed
# here to be done automatically.

load("@bazel_skylib//lib:paths.bzl", "paths")

def _to_quoted_comma_separated_list(args):
    """Return a list of strings as a shell-quoted, comma-separated string."""
    return "\"%s\"" % ",".join(args)

def get_ffx_assembly_inputs(fuchsia_toolchain):
    """Return the list of inputs needed to run `ffx assembly` commands.

    Args:
      fuchsia_toolchain: A fuchsia_toolchain() instance used to locate
         all host tools, including the 'ffx' one.
    Returns:
      A list of File instances.
    """

    # NOT: The 'ffx assembly' plugin used by this command requires access to
    # several host tools, and their location will be passed with explicit
    # overrides (see get_ffx_assembly_args()).
    return [
        fuchsia_toolchain.ffx,
        fuchsia_toolchain.ffx_assembly,
        fuchsia_toolchain.ffx_assembly_fho_meta,
        fuchsia_toolchain.sdk_manifest,
        fuchsia_toolchain.blobfs,
        fuchsia_toolchain.cmc,
        fuchsia_toolchain.fvm,
        fuchsia_toolchain.minfs,
        fuchsia_toolchain.zbi,
    ]

def get_ffx_assembly_args(fuchsia_toolchain):
    """Return the start of a command line used to call `ffx assembly`.

    Args:
      fuchsia_toolchain: A fuchsia_toolchain() instance used to locate
         all host tools, including the 'ffx' one.
    Returns:
      A list of string or File instances.
    """
    return [
        fuchsia_toolchain.ffx_assembly.path,
        "--config",
        _to_quoted_comma_separated_list([
            "sdk.overrides.assembly=" + fuchsia_toolchain.ffx_assembly.path,
            "sdk.overrides.blobfs=" + fuchsia_toolchain.blobfs.path,
            "sdk.overrides.cmc=" + fuchsia_toolchain.cmc.path,
            "sdk.overrides.fvm=" + fuchsia_toolchain.fvm.path,
            "sdk.overrides.minfs=" + fuchsia_toolchain.minfs.path,
            "sdk.overrides.zbi=" + fuchsia_toolchain.zbi.path,
        ]),
    ]

def get_ffx_product_inputs(fuchsia_toolchain):
    """Return the list of inputs needed to run `ffx product` commands.

    Args:
      fuchsia_toolchain: A fuchsia_toolchain() instance used to locate
         all host tools, including the 'ffx' one.
    Returns:
      A list of File instances.
    """
    return [
        fuchsia_toolchain.ffx_product,
        fuchsia_toolchain.ffx_product_fho_meta,
        fuchsia_toolchain.sdk_manifest,
        fuchsia_toolchain.blobfs,
    ]

def get_ffx_product_args(fuchsia_toolchain):
    return [
        fuchsia_toolchain.ffx_product.path,
        "--config",
        _to_quoted_comma_separated_list([
            # The SDK version is needed by `ffx product create`, so add the sdk info.
            "sdk.manifest=" + fuchsia_toolchain.sdk_manifest.path,
            "sdk.root=" + paths.dirname(fuchsia_toolchain.sdk_manifest.dirname),
            "sdk.overrides.blobfs=" + fuchsia_toolchain.blobfs.path,
        ]),
    ]

def get_ffx_scrutiny_inputs(fuchsia_toolchain):
    """Return the list of inputs needed to run `ffx scrutiny` commands.

    Args:
      fuchsia_toolchain: A fuchsia_toolchain() instance used to locate
         all host tools, including the 'ffx' one.
    Returns:
      A list of File instances.
    """
    return [
        fuchsia_toolchain.ffx_scrutiny,
        fuchsia_toolchain.ffx_scrutiny_fho_meta,
        fuchsia_toolchain.sdk_manifest,
    ]

def get_ffx_scrutiny_args(fuchsia_toolchain):
    return [
        fuchsia_toolchain.ffx_scrutiny.path,
    ]
