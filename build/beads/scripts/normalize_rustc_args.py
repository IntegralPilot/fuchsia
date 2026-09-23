# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Library to normalize rustc command arguments.
"""

import os
import shlex

import path_normalizer

# List of args to ignore when comparing GN and Bazel commands.
_ARGS_TO_IGNORE = (
    # Dependency directories and externs are provided by response files in GN,
    # so omit them.
    # This should be OK since missing these args would cause compilation to fail.
    "--extern",
    "-L",
    "-Ldependency",
    "-Zshell-argfiles",
    "@shell:",
    # Ignore --emit flags for now. In GN they are used to emit dep-info and
    # rmeta files, which Bazel doesn't need for now.
    "--emit=",
    "-Zdep-info-omit-d-target",
    # This is the default value, which Bazel sets explicitly, and GN omits.
    "--error-format=human",
    # GN and Bazel writes outputs to different locations, so ignore output flags.
    "--out-dir=",
    "-o=",
    # Bazel sets sysroot to the Rust toolchain in bazel-out, while GN omits this.
    "--sysroot=",
    # Bazel sets this for determinism purposes, and GN omits it.
    "--remap-path-prefix=",
    # Ignore remote-only flags, which are used in GN to maximize RBE cache hits
    # by utilizing wrapper scripts.
    "--remote-only",
    # API flags for rustc are handled differently in GN and Bazel.
    # GN relies on a generated rust_api_level_cfg_flags.txt, while Bazel sets them directly.
    "--cfg=fuchsia_api_level_at_least=",
    "--cfg=fuchsia_api_level_less_than=",
    "@rust_api_level_cfg_flags.txt",
    # TODO(https://fxbug.dev/477167250): Propagate debug_info to Bazel and
    # remove this.
    "-Cdebug-assertions=",
    "-Cdebuginfo=",
    # TODO(https://fxbug.dev/478707341): LTO and thinlto seems to cause
    # inconsistency here, figure out how to match the config between GN and
    # Bazel.
    "-Cembed-bitcode=",
    "-Ccodegen-units=16",
    # TODO(https://fxbug.dev/478707341): Figure out why the default value of
    # -Cstrip is different between GN and Bazel.
    "-Cstrip=",
    # TODO(https://fxbug.dev/478707341): Figure out why the default value of
    # -Copt-level is different between GN and Bazel.
    "-Copt-level=",
    # TODO(https://fxbug.dev/478707341): Figure out how to set the following args in Bazel and remove this.
    "--cfg=__rust_toolchain=",
    "-Cmetadata=",
    "RUST_BACKTRACE=1",
    # TODO(https://fxbug.dev/478707341): Figure out the root causes of link-arg inconsistencies.
    "-Clink-arg=",
    "-Clink-args=",
    # TODO(https://fxbug.dev/478707341): Figure out how to make remote flags
    # consistent between GN and Bazel.
    "--remote-flag=",
    # TODO(https://fxbug.dev/478707341): GN uses clang++ and Bazel uses clang.
    # Figure out where this discrepancy comes from and remove this.
    "-Clinker=",
    # TODO(https://fxbug.dev/478707341): Bazel adds this for some binaries.
    # Figure out why and determine if it's OK to ignore.
    "-Cextra-filename=",
)

# Argument prefixes that need to be converted for consistency between GN and Bazel.
_ARGS_PREFIX_CONVERSION_MAP = {
    "--codegen": "-C",
    "--allow": "-A",
    "--deny": "-D",
    "--warn": "-W",
    # Strip `--local-only` to get the actual args used in the build commands
    # when GN RBE mode is set to "local".
    "--local-only": "",
}

_ARGS_PREFIXES_TO_CONVERT = tuple(_ARGS_PREFIX_CONVERSION_MAP.keys())


def normalize_rustc_cmd(
    cmd: str, normalizer: path_normalizer.PathNormalizer
) -> list[str]:
    """Normalize a full rustc command.

    This function normalizes arguments by:
    - Omitting certain arguments that are not relevant to the comparison.
    - Converting some flags to a more common format.

    Args:
        cmd: The command to normalize.
        normalizer: A PathNormalizer instance.

    Returns:
        The normalized command.
    """

    # Fixups to the GN rustc command where it uses `--arg val` instead of
    # `--arg=val`, which differ from Bazel. These need to be done before
    # tokenizing the command line.
    rustc_cmd_replaced = cmd.replace("--target ", "--target=").replace(
        "-o ", "-o="
    )
    return sorted(
        set(
            normalize_rustc_arg(a, normalizer=normalizer)
            for a in shlex.split(rustc_cmd_replaced)
        )
    )


def normalize_rustc_arg(
    arg: str,
    normalizer: path_normalizer.PathNormalizer,
) -> str:
    """Normalize a single rustc argument.

    This function normalizes arguments by:
    - Omitting certain arguments that are not relevant to the comparison.
    - Converting some flags to a more common format.
    - Normalizing the paths prefix into common expression such as {SOURCE_DIR}.

    Args:
        arg: The argument to normalize.
        normalizer: A PathNormalizer instance.

    Returns:
        The normalized argument.
    """
    # Convert to the same flag format.
    if arg.startswith(_ARGS_PREFIXES_TO_CONVERT):
        opt, _, val = arg.partition("=")
        opt_new = _ARGS_PREFIX_CONVERSION_MAP[opt]
        arg = f"{opt_new}{val}"

    if arg.startswith(_ARGS_TO_IGNORE):
        return ""

    if arg.startswith("-Clinker="):
        parts = arg.split("=", maxsplit=1)
        base_linker_name = os.path.basename(parts[1])
        return f"-Clinker={base_linker_name}"

    if not arg.startswith("-"):
        # This is likely a path, e.g. not a `--arg=val` argument, so try to
        # apply path-related normalization.

        # Normalize paths to the `rustc` compiler.
        if os.path.basename(arg) == "rustc":
            return "rustc"

        # Ignore Bazel-specific paths for prebuilt rust toolchain lib.
        if arg.startswith("bazel-out") and "fuchsia_prebuilt_rust" in arg:
            return ""

        # Try to normalize relative/absolute source path
        try:
            return normalizer.normalize_path(arg)
        except ValueError:
            pass

    return arg
