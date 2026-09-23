# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Registers debug symbols with ffx as a task workflow."""

load(
    "@fuchsia_rules_common//debug_symbols:debug_symbols.bzl",
    "merge_debug_symbol_infos",
)
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":fuchsia_shell_task.bzl", "shell_task_rule")

def _fuchsia_task_register_debug_symbols_impl(ctx, make_shell_task):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    # The symbol-index.json file uses the "build_dir" key for source search roots.
    build_id_dirs, build_dirs = zip(*[
        (build_id_dir, source_search_root)
        for source_search_root, build_id_dirs_depset in merge_debug_symbol_infos(ctx.attr.deps).build_id_dirs_mapping.items()
        for build_id_dir in build_id_dirs_depset.to_list()
    ]) or ([], [])

    return make_shell_task(
        command = [
            ctx.attr._tool,
            "--ffx",
            sdk.ffx,
            "--build-id-dirs",
        ] + list(build_id_dirs) + [
            "--build-dirs",
        ] + list(build_dirs),
    )

(
    _fuchsia_task_register_debug_symbols,
    _fuchsia_task_register_debug_symbols_for_test,
    fuchsia_task_register_debug_symbols,
) = shell_task_rule(
    doc = """Registers debug symbols with ffx.""",
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    implementation = _fuchsia_task_register_debug_symbols_impl,
    attrs = {
        "_tool": attr.label(
            doc = "The tool needed to register debug symbols.",
            default = "//fuchsia/tools:register_debug_symbols",
            executable = True,
            cfg = "exec",
        ),
        "deps": attr.label_list(
            doc = """Collects FuchsiaDebugSymbolInfo across multiple dependencies.
            If a dependency does not provide the FuchsiaDebugSymbolInfo it will be ignored.
            """,
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
