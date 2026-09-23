# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Implement fuchsia_structured_config() rule.

(This is a translation of the C++ templates defined in
//build/components/fuchsia_structured_config.gni.)
"""

load(
    "@fuchsia_rules_common//:utils.bzl",
    "make_resource_struct",
)
load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load(":fuchsia_fidl_cc_library.bzl", "fuchsia_fidl_cc_library")
load(":fuchsia_fidl_library.bzl", "fuchsia_fidl_library")
load(":fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(
    ":providers.bzl",
    "FuchsiaComponentManifestInfo",
    "FuchsiaPackageResourcesInfo",
    "FuchsiaStructuredConfigInfo",
)

#####
# cvf
#####

def _cvf_impl(ctx):
    compiled_output = ctx.actions.declare_file(ctx.attr.name + ".cvf")
    sdk = get_fuchsia_sdk_toolchain(ctx)
    ctx.actions.run(
        executable = sdk.configc,
        arguments = [
            "cvf",
            "--cm",
            ctx.file.cm_label.path,
            "--values",
            ctx.file.value_file.path,
            "--output",
            compiled_output.path,
        ],
        inputs = [ctx.file.cm_label, ctx.file.value_file],
        outputs = [compiled_output],
        mnemonic = "ConfigcCVF",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )

    resources = [
        make_resource_struct(
            src = compiled_output,
            dest = ctx.attr.cm_label[FuchsiaComponentManifestInfo].config_package_path,
        ),
    ]

    return [
        FuchsiaPackageResourcesInfo(resources = resources),
        DefaultInfo(files = depset([compiled_output])),
        FuchsiaStructuredConfigInfo(
            cvf_source = compiled_output,
            cvf_dest = ctx.attr.cm_label[FuchsiaComponentManifestInfo].config_package_path,
        ),
    ]

_cvf = rule(
    doc = """Compile a configuration value file.

      (This is a translation of the cvf template defined in
      //tools/configc/build/config.gni.)
    """,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    implementation = _cvf_impl,
    attrs = {
        "cm_label": attr.label(
            doc = """Target that generates the compiled manifest,
            for which the value file should be compiled.""",
            allow_single_file = True,
            mandatory = True,
        ),
        "value_file": attr.label(
            doc = "A JSON5 file containing the configuration values to be compiled.",
            allow_single_file = True,
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

###############################
# fidl_config_client_lib_source
###############################

def _fidl_config_client_lib_source_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    source_file = ctx.actions.declare_file(ctx.attr.name + ".fidl")
    ctx.actions.run(
        executable = sdk.configc,
        arguments = [
            "fidl",
            "--cm",
            ctx.file.cm_label.path,
            "--output",
            source_file.path,
            "--library-name",
            ctx.attr.fidl_name,
            "--fidl-format",
            sdk.fidl_format.path,
        ],
        inputs = [ctx.file.cm_label, sdk.fidl_format],
        outputs = [source_file],
        mnemonic = "FidlClientLibSource",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )
    return DefaultInfo(files = depset([source_file]))

_fidl_config_client_lib_source = rule(
    doc = """Generate the FIDL client library source files for a configuration declaration.

      (This is a translation of the same-named template defined in
      //tools/configc/build/config.gni.)
    """,
    implementation = _fidl_config_client_lib_source_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "cm_label": attr.label(
            doc = """Target that generates the compiled manifest,
            for which the source files should be generated.""",
            mandatory = True,
            allow_single_file = True,
        ),
        "fidl_name": attr.string(
            doc = """Name for the generated FIDL library.""",
            mandatory = True,
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

###############################
# cpp_config_client_lib_source
###############################

def _cpp_config_client_lib_source_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    namespace = ctx.attr.namespace if ctx.attr.namespace else ctx.attr.name
    cc_source_file = ctx.actions.declare_file(ctx.attr.namespace + ".cc")
    h_source_file = ctx.actions.declare_file(ctx.attr.namespace + ".h")
    ctx.actions.run(
        executable = sdk.configc,
        arguments = [
            "cpp",
            "--cm",
            ctx.file.cm_label.path,
            "--h-output",
            h_source_file.path,
            "--cc-output",
            cc_source_file.path,
            "--namespace",
            namespace,
            "--fidl-library-name",
            ctx.attr.fidl_library_name,
            "--clang-format",
            ctx.executable._clang_format.path,
        ],
        inputs = [ctx.file.cm_label, ctx.executable._clang_format],
        outputs = [h_source_file, cc_source_file],
        mnemonic = "ConfigcClientLibSource",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )
    return DefaultInfo(files = depset([h_source_file, cc_source_file]))

_cpp_config_client_lib_source = rule(
    doc = """Generate the C++ client library source files for a configuration declaration.

      (This is a translation of the same-named template defined in
      //tools/configc/build/config.gni.)
    """,
    implementation = _cpp_config_client_lib_source_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "cm_label": attr.label(
            doc = """Target that generates the compiled manifest,
            for which the source files should be generated.""",
            mandatory = True,
            allow_single_file = True,
        ),
        "fidl_library_name": attr.string(
            doc = "Name for the internal FIDL library.",
            mandatory = True,
        ),
        "namespace": attr.string(
            doc = "Namespace used by the C++ library.",
        ),
        "_flavor": attr.string(
            doc = "Runner flavor for client library.",
            default = "elf",
        ),
        "_clang_format": attr.label(
            doc = "clang-format tool.",
            cfg = "exec",
            executable = True,
            allow_single_file = True,
            default = "@fuchsia_clang//:bin/clang-format",
        ),
    } | COMPATIBILITY.FUCHSIA_ATTRS,
)

##################################
# fuchsia_structured_config_values
##################################

def fuchsia_structured_config_values(
        name,
        cm_label,
        values = None,
        values_source = None,
        component_name = "",
        # FIXME(https://fxbug.dev/364917537): `cvf_output_name` is not used meaningfully here.
        # It should either be plumbed or deleted.
        cvf_output_name = ""):
    """Defines a configuration value file for a Fuchsia component.

    Args:
      name: Target name. Required.
      cm_label: Target that generates the component manifest. Required.
      values_source: The JSON5 file containing the concrete values for the generated file.
        This must not be set if using `values`.
      values: a starlark dictionary containing literal values for the generated file.
        This must not be set if using `values_source`.
      component_name: The basename of the component manifest within the package's meta/ dir.
      cvf_output_name: The name of the cvf file that is being produced.
    """
    if (not values_source) == (not values):
        fail("Exactly one of \"values\" or \"values_source\" must be specified.")

    # FIXME(https://fxbug.dev/364917537): `_value_file_deps` is not used meaningfully here.
    # It should either be plumbed or deleted.
    _value_file_deps = []  # buildifier: disable=unused-variable

    _value_file = values_source
    if values:
        _generated_values_label = "%s_generated_values" % name
        _value_file_deps = [":" + _generated_values_label]
        _value_file = "%s_values_from_literal.json" % name
        _json_string = json.encode(values).replace("\"", "\\\"")
        native.genrule(
            name = _generated_values_label,
            outs = [_value_file],
            cmd = "echo \"%s\" > $@" % _json_string,
        )

    # buildifier: disable=unused-variable
    _cvf_output_name = component_name
    if cvf_output_name:
        _cvf_output_name = cvf_output_name

    # compile the value file
    _cvf(
        name = name,
        cm_label = cm_label,
        value_file = _value_file,
    )

#######################################
# fuchsia_structured_config_cpp_elf_lib
#######################################

def fuchsia_structured_config_cpp_elf_lib(
        name,
        cm_label,
        namespace = "",
        fidl_library_name = "cf.sc.internal",
        tags = ["manual"],
        **kwargs):
    """Defines a C++ configuration client library for a Fuchsia ELF component.

    Args:
      name: Target name. Required.
      cm_label: Target that generates the component manifest. Required.
      namespace: Namespace used by the generated C++ library.
        If not specified, the target name is used.
      fidl_library_name: Name of the generated FIDL library.
        If not specified, the default (cf.sc.internal) is used.
      tags: Typical bazel semantic.
      **kwargs: Additional common bazel kwargs to forward to all rules.
    """

    if not cm_label:
        fail("Must provide a component manifest label")

    if not namespace:
        namespace = name
    namespace = namespace.replace(".", "_").replace("-", "_")

    # generate the client library FIDL source
    fidl_source_target = "%s_fidl_config_lib_source" % name
    _fidl_config_client_lib_source(
        name = fidl_source_target,
        cm_label = cm_label,
        fidl_name = fidl_library_name,
        tags = tags,
        **kwargs
    )

    # generate the C++ source
    cpp_elf_source_target = "%s_cpp_elf_config_lib_source" % name
    _cpp_config_client_lib_source(
        name = cpp_elf_source_target,
        namespace = namespace,
        fidl_library_name = fidl_library_name,
        cm_label = cm_label,
        tags = tags,
        **kwargs
    )

    # generate the FIDL library
    fidl_library_target = "%s_fidl_internal" % name
    fuchsia_fidl_library(
        name = fidl_library_target,
        srcs = [fidl_source_target],
        library = fidl_library_name,
        cc_bindings = ["cpp"],
        tags = tags,
        **kwargs
    )

    cc_bind_target = "%s_bindlib_cc" % fidl_library_name
    fuchsia_fidl_cc_library(
        name = cc_bind_target,
        binding_type = "cpp",
        library = ":" + fidl_library_target,
        tags = tags,
        **kwargs
    )

    cc_library(
        name = name,
        srcs = [":" + cpp_elf_source_target],
        deps = [
            ":" + cc_bind_target,
            "@fuchsia_sdk//pkg/inspect",
        ],
        tags = tags,
        **kwargs
    )
