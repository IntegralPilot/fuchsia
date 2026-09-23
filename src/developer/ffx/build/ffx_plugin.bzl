# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Starlark rules for ffx plugin dynamic code generation (Theme A)."""

load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load("//build/bazel/rules/rust:defs.bzl", "rustc_library")
load("//build/tools/json_merge:json_merge.bzl", "json_merge")

def _ffx_generate_cmd_impl(ctx):
    out_file = ctx.actions.declare_file(ctx.attr.output_name)
    args = ctx.actions.args()
    args.add("--out", out_file.path)
    args.add("--deps", ",".join(ctx.attr.crate_names))

    ctx.actions.run(
        outputs = [out_file],
        inputs = [],
        executable = ctx.executable._tool,
        arguments = [args],
        mnemonic = "FfxGenCmd",
        progress_message = "Generating command args for %s" % ctx.label,
    )
    return [DefaultInfo(files = depset([out_file]))]

_ffx_generate_cmd = rule(
    implementation = _ffx_generate_cmd_impl,
    attrs = {
        "output_name": attr.string(mandatory = True),
        "crate_names": attr.string_list(mandatory = True),
        "_tool": attr.label(
            executable = True,
            cfg = "exec",
            default = "//src/developer/ffx/build:gn_generate_cmd",
        ),
    },
)

def _ffx_generate_plugins_impl(ctx):
    out_file = ctx.actions.declare_file(ctx.attr.output_name)
    args = ctx.actions.args()
    args.add("--out", out_file.path)
    args.add("--args", ctx.attr.args_lib)
    args.add("--execution_lib", ctx.attr.execution_lib)
    if ctx.attr.includes_execution:
        args.add("--includes_execution")
    if ctx.attr.includes_subcommands:
        args.add("--includes_subcommands")
    if ctx.attr.plugin_crate_names:
        args.add("--deps", ",".join(ctx.attr.plugin_crate_names))
    if ctx.attr.sub_command_lib:
        args.add("--sub_command", ctx.attr.sub_command_lib)

    ctx.actions.run(
        outputs = [out_file],
        inputs = [],
        executable = ctx.executable._tool,
        arguments = [args],
        mnemonic = "FfxGenPlugins",
        progress_message = "Generating plugins registry for %s" % ctx.label,
    )
    return [DefaultInfo(files = depset([out_file]))]

_ffx_generate_plugins = rule(
    implementation = _ffx_generate_plugins_impl,
    attrs = {
        "output_name": attr.string(mandatory = True),
        "args_lib": attr.string(mandatory = True),
        "execution_lib": attr.string(mandatory = True),
        "includes_execution": attr.bool(default = False),
        "includes_subcommands": attr.bool(default = False),
        "plugin_crate_names": attr.string_list(),
        "sub_command_lib": attr.string(),
        "_tool": attr.label(
            executable = True,
            cfg = "exec",
            default = "//src/developer/ffx/build:gn_generate_plugins",
        ),
    },
)

def _ffx_plugin_impl(
        name,
        args_sources,
        args_deps = [],
        sources = [],
        deps = [],
        proc_macro_deps = [],
        plugin_deps = [],
        config_data = [],
        with_unit_tests = False,
        edition = "2024",
        sdk_category = None,
        version = None,
        visibility = None):
    _crate_name = name.replace("-", "_")

    # TODO(https://fxbug.dev/515620287): Support SDK category and compatibility enforcements.
    # GN version has complex logic for sdk_category and category_marker.
    if sdk_category:
        pass

    # TODO(https://fxbug.dev/515620287): Support unit tests.

    # Subcommand library
    if plugin_deps:
        _ffx_generate_cmd(
            name = name + "_sub_command_gen",
            output_name = name + "_cmd_args.rs",
            crate_names = [Label(d).name for d in plugin_deps],
        )

        rustc_library(
            name = name + "_sub_command",
            srcs = [":" + name + "_sub_command_gen"],
            crate_name = _crate_name + "_sub_command",
            edition = edition,
            deps = [
                "//third_party/rust_crates/vendor:argh",
            ] + [str(d) + "_args" for d in plugin_deps],
            target_compatible_with = HOST_CONSTRAINTS,
            visibility = visibility,
        )

    # Configuration merging
    config_srcs = config_data
    if not config_srcs:
        native.genrule(
            name = name + "_empty_config_json",
            outs = [name + "_empty_config.json"],
            cmd = "echo '{}' > $@",
        )
        config_srcs = [":" + name + "_empty_config_json"]
    for dep in plugin_deps:
        config_target_name = Label(dep).name + "_config"
        config_srcs.append(Label(dep).same_package_label(config_target_name))

    json_merge(
        name = name + "_config",
        srcs = config_srcs,
        minify = True,
        visibility = visibility,
    )

    # Args library
    rustc_library(
        name = name + "_args",
        srcs = args_sources,
        crate_name = _crate_name + "_args",
        edition = edition,
        deps = args_deps + ([":" + name + "_sub_command"] if plugin_deps else []),
        visibility = visibility,
        version = version,
        target_compatible_with = HOST_CONSTRAINTS,
    )

    # Execution library
    if sources:
        rustc_library(
            name = name,
            srcs = sources,
            crate_name = _crate_name,
            edition = edition,
            deps = deps + [":" + name + "_args"],
            proc_macro_deps = proc_macro_deps,
            with_host_unit_tests = with_unit_tests,
            target_compatible_with = HOST_CONSTRAINTS,
        )

    # Plugins generation
    _ffx_generate_plugins(
        name = name + "_plugins_gen",
        args_lib = _crate_name + "_args",
        execution_lib = _crate_name,
        includes_execution = bool(sources),
        includes_subcommands = bool(plugin_deps),
        plugin_crate_names = [Label(d).name for d in plugin_deps],
        sub_command_lib = _crate_name + "_sub_command" if plugin_deps else None,
        output_name = name + "_plugins.rs",
    )

    # Suite library
    suite_deps = [
        ":" + name + "_args",
        "//src/developer/ffx/lib/fho:lib",
    ]
    if sources:
        suite_deps.append(":" + name)
    if plugin_deps:
        suite_deps.append(":" + name + "_sub_command")
        for dep in plugin_deps:
            suite_deps.append(str(dep) + "_suite")

    rustc_library(
        name = name + "_suite",
        srcs = [":" + name + "_plugins_gen"],
        crate_name = _crate_name + "_suite",
        edition = edition,
        deps = suite_deps,
        data = [":" + name + "_plugins_gen"],
        version = version,
        visibility = visibility,
        target_compatible_with = HOST_CONSTRAINTS,
    )

ffx_plugin = macro(
    doc = """Defines an FFX plugin.

This macro generates the necessary targets for an FFX plugin.

Public subtargets created and exposed:

  - `{name}_suite` (Always): The main execution entry point for the plugin.
  - `{name}_args` (Always): Compiles the CLI arguments parser (usually `src/args.rs`).
  - `{name}_config` (Always): Merges configuration data from `config_data` and all `plugin_deps`.

Other internal-only targets (such as the raw `{name}` execution library, subcommand generators,
and intermediate code registries) are created as private implementation details and are not exposed
outside of the macro to maintain strict encapsulation.
""",
    implementation = _ffx_plugin_impl,
    attrs = {
        "args_sources": attr.label_list(
            doc = "List of source files for the args library.",
            mandatory = True,
            allow_files = True,
        ),
        "args_deps": attr.label_list(
            doc = "List of targets on which the args library depends.",
            default = [],
        ),
        "sources": attr.label_list(
            doc = "List of source files for the plugin library.",
            default = [],
            allow_files = True,
            configurable = False,
        ),
        "deps": attr.label_list(
            doc = "List of targets on which the plugin library depends.",
            default = [],
        ),
        "proc_macro_deps": attr.label_list(
            doc = "List of proc macro targets on which the plugin library depends.",
            default = [],
        ),
        "plugin_deps": attr.label_list(
            doc = "List of subcommand plugin targets.",
            default = [],
            configurable = False,
        ),
        "config_data": attr.label_list(
            doc = "List of JSON files containing configuration data.",
            default = [],
            allow_files = True,
            configurable = False,
        ),
        "with_unit_tests": attr.bool(
            doc = "Builds unit tests associated with the library.",
            default = False,
            configurable = False,
        ),
        "edition": attr.string(
            doc = "Edition of the Rust language to be used.",
            default = "2024",
        ),
        "sdk_category": attr.string(
            doc = "SDK category for the plugin.",
        ),
        "version": attr.string(
            doc = "Crate version.",
        ),
    },
)
