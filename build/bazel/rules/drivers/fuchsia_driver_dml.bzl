# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules for generating driver bindings and manifests from DML."""

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("@rules_fuchsia//fuchsia:defs.bzl", "COMPATIBILITY")

visibility([
    "//src/devices/...",
    "//vendor/...",
])

_DML_ATTRS = {
    "dml": attr.label(
        mandatory = True,
        allow_single_file = True,
        doc = "Path to the DML file.",
    ),
    "driver_name": attr.string(
        mandatory = True,
        doc = "Name of the driver (used for parser class name and file names).",
    ),
}

def _generate_dml_cc_sources_impl(ctx):
    driver_name = ctx.attr.driver_name
    dml = ctx.file.dml

    bind = ctx.actions.declare_file(driver_name + ".bind")
    cml = ctx.actions.declare_file(driver_name + ".cml")
    parser_h = ctx.actions.declare_file(driver_name + "_parser.h")
    parser_cc = ctx.actions.declare_file(driver_name + "_parser.cc")
    outputs = [bind, cml, parser_h, parser_cc]

    ctx.actions.run(
        executable = ctx.executable._dmlc,
        inputs = [dml],
        outputs = outputs,
        arguments = [
            "compile-driver",
            dml.path,
            "--bind-output",
            bind.path,
            "--cml-output",
            cml.path,
            "--h-output",
            parser_h.path,
            "--cc-output",
            parser_cc.path,
        ],
        mnemonic = "DmlCompiler",
    )

    return [
        DefaultInfo(
            files = depset(outputs),
        ),
        OutputGroupInfo(
            bind = depset([bind]),
            cml = depset([cml]),
            parser_headers = depset([parser_h]),
            parser_sources = depset([parser_cc]),
        ),
    ]

_generate_dml_cc_sources = rule(
    implementation = _generate_dml_cc_sources_impl,
    attrs = _DML_ATTRS | {
        "_dmlc": attr.label(
            default = "@gn_targets//toolchain_host_x64/src/devices/tools/dmlc:dmlc",
            executable = True,
            cfg = "exec",
        ),
    } | COMPATIBILITY.FUCHSIA_ATTRS,
)

def _dml_cc_library_impl(
        name,
        dml,
        driver_name,
        testonly,
        visibility,
        deps,
        includes,
        **kwargs):
    dml_gen_name = name + "_gen"
    _generate_dml_cc_sources(
        name = dml_gen_name,
        dml = dml,
        driver_name = driver_name,
        testonly = testonly,
    )

    # Export helper targets with the same visibility as the cc_library
    # so they can be depended on directly (e.g. by driver components).
    filegroup_visibility = visibility

    native.filegroup(
        name = name + "_bind",
        srcs = [":" + dml_gen_name],
        output_group = "bind",
        testonly = testonly,
        visibility = filegroup_visibility,
    )

    native.filegroup(
        name = name + "_cml",
        srcs = [":" + dml_gen_name],
        output_group = "cml",
        testonly = testonly,
        visibility = filegroup_visibility,
    )

    # These are only used internally by the cc_library target.
    private_visibility = ["//visibility:private"]

    native.filegroup(
        name = name + "_parser_hdrs",
        srcs = [":" + dml_gen_name],
        output_group = "parser_headers",
        testonly = testonly,
        visibility = private_visibility,
    )

    native.filegroup(
        name = name + "_parser_srcs",
        srcs = [":" + dml_gen_name],
        output_group = "parser_sources",
        testonly = testonly,
        visibility = private_visibility,
    )

    full_deps = [
        "@fuchsia_sdk//fidl/fuchsia.driver.metadata:fuchsia.driver.metadata_cpp",
        "@fuchsia_sdk//pkg/driver_metadata_cpp",
    ] + (deps or [])

    full_includes = ["."] + (includes or [])

    cc_library(
        name = name,
        srcs = [":" + name + "_parser_srcs"],
        hdrs = [":" + name + "_parser_hdrs"],
        includes = full_includes,
        deps = full_deps,
        testonly = testonly,
        visibility = visibility,
        **kwargs
    )

dml_cc_library = macro(
    doc = """Generates C++ parser, bind rules, and component manifest from a DML file.

    The main target created by this macro is a `cc_library` with the given `name`.
    It also creates helper `filegroup` targets for the bind rules and manifest:
    - `<name>_bind`: The generated `.bind` file.
    - `<name>_cml`: The generated `.cml` file.
    """,
    implementation = _dml_cc_library_impl,
    # TODO(https://fxbug.dev/446694542): Remove `native.` once the
    # rules_cc provides `cc_library()` as a symbolic macro.
    inherit_attrs = native.cc_library,
    attrs = _DML_ATTRS | {
        # Do not inherit as these are specified in the implementation.
        "srcs": None,
        "hdrs": None,
    },
)
