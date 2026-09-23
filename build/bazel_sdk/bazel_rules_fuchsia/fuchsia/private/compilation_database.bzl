# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Clang compilation database support."""

load(
    "@bazel_tools//tools/build_defs/cc:action_names.bzl",
    "CPP_COMPILE_ACTION_NAME",
    "C_COMPILE_ACTION_NAME",
)

# buildifier: disable=module-docstring
load("@bazel_tools//tools/cpp:toolchain_utils.bzl", "find_cpp_toolchain")
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/common:cc_info.bzl", "CcInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION")

_CC_RULES = [
    "cc_library",
    "cc_binary",
    "cc_test",
    "cc_inc_library",
    "cc_proto_library",
]

_CPP_HEADER_EXT = [
    "hpp",
    "hxx",
    "hh",
    "ipp",
]

_HEADER_EXT = _CPP_HEADER_EXT + ["h"]

_CPP_SRC_EXT = [
    "cpp",
    "cxx",
    "cc",
]

_CPP_EXT = _CPP_SRC_EXT + _CPP_HEADER_EXT

# Placeholder for exec root directory.
_EXEC_ROOT_MARKER = "__EXEC_ROOT__"

# Assume C++ if all files are headers or there is a c++ file.
def _is_cpp_target(srcs):
    if any([src.extension in _CPP_EXT for src in srcs]):
        return True
    if all([src.extension in _HEADER_EXT for src in srcs]):
        return True
    return False

def _get_sources(ctx):
    srcs = []
    if hasattr(ctx.rule.attr, "srcs"):
        srcs += [f for src in ctx.rule.attr.srcs for f in src.files.to_list()]
    if hasattr(ctx.rule.attr, "hdrs"):
        srcs += [f for src in ctx.rule.attr.hdrs for f in src.files.to_list()]

    return srcs

def _add_compile_flags(options, flag, args):
    for arg in args.to_list():
        if len(arg) == 0:
            arg = "."
        options.append("{} {}".format(flag, arg))

# Get compilation options from the CompilationContext from CcInfo
# https://docs.bazel.build/versions/main/skylark/lib/CcInfo.html
def _get_compile_flags(dep):
    options = []
    compilation_context = dep[CcInfo].compilation_context

    # ignore headers and validation_artifacts.
    _add_compile_flags(options, "-I", compilation_context.includes)
    _add_compile_flags(options, "-isystem", compilation_context.system_includes)
    _add_compile_flags(options, "-iquote", compilation_context.quote_includes)
    _add_compile_flags(options, "-F", compilation_context.framework_includes)
    _add_compile_flags(options, "-D", compilation_context.defines)
    _add_compile_flags(options, "-D", compilation_context.local_defines)

    return options

CompilationAspectInfo = provider(
    doc = "A private provider to pass information from a package build to archive.",
    fields = {
        "compilation_db": "compilation database",
    },
)

# Returns the command lines to compile all the c/c++ source files for the given target.
def _cc_compile_commands(ctx, target, feature_configuration, cc_toolchain):
    compile_flags = _get_compile_flags(target)

    #Assume C++ code
    srcs = _get_sources(ctx)

    if _is_cpp_target(srcs):
        compile_variables = cc_common.create_compile_variables(
            feature_configuration = feature_configuration,
            cc_toolchain = cc_toolchain,
            user_compile_flags = ctx.fragments.cpp.cxxopts +
                                 ctx.fragments.cpp.copts,
            add_legacy_cxx_options = True,
        )
        action_name = CPP_COMPILE_ACTION_NAME
    else:
        compile_variables = cc_common.create_compile_variables(
            feature_configuration = feature_configuration,
            cc_toolchain = cc_toolchain,
            user_compile_flags = ctx.fragments.cpp.copts,
        )
        action_name = C_COMPILE_ACTION_NAME

    compiler_options = cc_common.get_memory_inefficient_command_line(
        feature_configuration = feature_configuration,
        action_name = action_name,
        variables = compile_variables,
    )

    if hasattr(ctx.rule.attr, "copts"):
        compile_flags.extend(ctx.rule.attr.copts)

    compiler_info = str(
        cc_common.get_tool_for_action(
            feature_configuration = feature_configuration,
            action_name = action_name,
        ),
    )
    cmdline_list = [compiler_info]
    cmdline_list.extend(compiler_options)
    cmdline_list.extend(compile_flags)
    cmdline = " ".join(cmdline_list)

    compile_commands = []
    for src in srcs:
        compile_commands.append(struct(
            cmdline = cmdline + " -c " + src.path,
            src = src,
        ))
    return compile_commands

# Returns a list of compilation_db object entries for the given target.
def _build_compilation_db(target, ctx):
    compilation_db = []
    cc_toolchain = find_cpp_toolchain(ctx)
    feature_configuration = cc_common.configure_features(
        ctx = ctx,
        cc_toolchain = cc_toolchain,
        requested_features = ctx.features,
        unsupported_features = ctx.disabled_features,
    )

    if ctx.rule.kind in _CC_RULES:
        compile_commands = _cc_compile_commands(ctx, target, feature_configuration, cc_toolchain)
    else:
        return []

    for compile_command in compile_commands:
        compilation_db.append(
            struct(
                command = compile_command.cmdline,
                directory = _EXEC_ROOT_MARKER,
                file = compile_command.src.path,
            ),
        )

    return compilation_db

def _compilation_db_aspect_impl(target, ctx):
    transitive_compilation_db = []

    # Applies the aspect to the targets in the deps attribute, if present.
    if hasattr(ctx.rule.attr, "deps"):
        for dep in ctx.rule.attr.deps:
            transitive_compilation_db.append(dep[CompilationAspectInfo].compilation_db)

    if hasattr(ctx.rule.attr, "content"):
        for dep in ctx.rule.attr.content:
            if CompilationAspectInfo in dep:
                transitive_compilation_db.append(dep[CompilationAspectInfo].compilation_db)

    compilation_db = depset(
        _build_compilation_db(target, ctx),
        transitive = transitive_compilation_db,
    )

    return [CompilationAspectInfo(compilation_db = compilation_db)]

compilation_db_aspect = aspect(
    implementation = _compilation_db_aspect_impl,
    attr_aspects = ["deps", "content"],
    attrs = {
        "_cc_toolchain": attr.label(
            default = Label("@bazel_tools//tools/cpp:current_cc_toolchain"),
        ),
    },
    fragments = ["cpp"],
    toolchains = ["@bazel_tools//tools/cpp:toolchain_type"],
)

def _compilation_db_rule_impl(ctx):
    compilation_db = []
    for dep in ctx.attr.deps:
        compilation_db.append(dep[CompilationAspectInfo].compilation_db)

    compilation_db = depset(transitive = compilation_db)

    exec_root = ctx.attr.output_base + "/execroot/" + ctx.workspace_name

    content = json.encode(compilation_db.to_list())
    content = content.replace(_EXEC_ROOT_MARKER, exec_root)

    ctx.actions.write(output = ctx.outputs.filename, content = content)

_clangd_compilation_database = rule(
    implementation = _compilation_db_rule_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "output_base": attr.string(
            default = "__OUTPUT_BASE__",
            doc = ("Output Bazel directory to store the compilation database"),
        ),
        "deps": attr.label_list(aspects = [compilation_db_aspect]),
        "filename": attr.output(),
    } | COMPATIBILITY.HOST_ATTRS,
)

# TODO(https://fxbug.dev/42174048): Revisit the compilation database implementation after a review of alternatives
def clangd_compilation_database(**kwargs):
    _clangd_compilation_database(
        filename = kwargs.pop("filename", "compile_commands.json"),
        **kwargs
    )
