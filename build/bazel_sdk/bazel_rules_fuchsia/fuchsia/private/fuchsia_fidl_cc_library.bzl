# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A cc_library backed by a FIDL library."""

load("@rules_cc//cc:cc_library.bzl", "cc_library")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":providers.bzl", "FuchsiaFidlLibraryInfo")

_CodegenInfo = provider("Carries generated information across FIDL bindings code generation ", fields = ["files"])

def _typed_deps(deps, binding_type):
    result = []
    for dep in deps:
        if ":" in dep:  # appends the binding_type suffix
            dep_with_binding = get_cc_lib_name(dep, binding_type)
        else:
            # assumption: this is a path with an implicit target name
            # TODO: THIS WILL FAIL eventually with local targets without ":" for example.
            # A better implementation is needed! We can't use Label() here because this is evaluated
            # in the generator ctx, not the SDK ctx.
            # One possible better implementation is to make fuchsia_fidl carry all the transitive dependencies per
            # cc binding type, and provide them as a Provider, and then just collect them in _codegen and make them
            # _codegen's own dependencies, which would probably cause cc_library to do the right thing.
            dep_label = dep.rpartition("/")[2]
            dep_label = get_cc_lib_name(dep_label, binding_type)
            dep_with_binding = "%s:%s" % (dep, dep_label)

        result.append(dep_with_binding)
    return result

def get_cc_lib_name(fidl_target_name, binding_type):
    return "{fidl}_{binding_type}".format(fidl = fidl_target_name, binding_type = binding_type)

def fuchsia_fidl_cc_library(name, library, binding_type = "cpp_wire", deps = [], tags = [], **kwargs):
    """Generates cc_library() for the given fidl_library.

    Args:
      name: Target name. Required.
      library: fidl_library() target to generate the language bindings for. Required.
      binding_type: the FIDL binding type, for example "cpp_wire", "cpp_driver" or "cpp_natural_types". Defaults to "cpp_wire"
      deps: Additional dependencies.
      tags: Optional tags.
      **kwargs: Remaining args.
    """
    gen_name = "%s_codegen" % name
    impl_name = "%s_impl" % name

    _codegen(
        name = gen_name,
        library = library,
        binding_type = binding_type,
        **kwargs
    )

    _impl_wrapper(
        name = impl_name,
        codegen = ":%s" % gen_name,
        **kwargs
    )

    cc_library(
        name = name,
        hdrs = [
            ":%s" % gen_name,
        ],
        srcs = [
            ":%s" % impl_name,
        ],
        # This is necessary in order to locate generated headers.
        strip_include_prefix = gen_name + "." + binding_type,
        deps = _typed_deps(deps, binding_type) + [
            "@fuchsia_sdk//pkg/fidl_cpp_wire",
            "@fuchsia_sdk//pkg/fidl_cpp_v2",
            "@fuchsia_sdk//pkg/fidl_driver",
            "@fuchsia_sdk//pkg/fidl_driver_natural",
        ],
        tags = tags,
        **kwargs
    )

def _codegen_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    ir = ctx.attr.library[FuchsiaFidlLibraryInfo].ir
    name = ctx.attr.library[FuchsiaFidlLibraryInfo].name

    base_path = ctx.attr.name + "." + ctx.attr.binding_type

    header_files = [
        "common_types_format.h",
        "common_types.h",
        "driver/natural_messaging.h",
        "driver/wire_messaging.h",
        "driver/wire.h",
        "driver/fidl.h",
        "fidl.h",
        "hlcpp_conversion.h",
        "markers.h",
        "natural_messaging.h",
        "natural_ostream.h",
        "natural_types.h",
        "test_base.h",
        "type_conversions.h",
        "wire_messaging.h",
        "wire_test_base.h",
        "wire_types.h",
        "wire.h",
    ]
    source_files = [
        "common_types.cc",
        "driver/wire_messaging.cc",
        "driver/natural_messaging.cc",
        "natural_messaging.cc",
        "natural_ostream.cc",
        "natural_types.cc",
        "type_conversions.cc",
        "wire_messaging.cc",
        "wire_types.cc",
    ]

    # TODO(https://fxbug.dev/42060065): Better workaround for skipping codegen for zx.
    if name == "zx":
        source_files = ["markers.h"]

    subdir = "fidl/" + name + "/cpp"

    headers = [
        ctx.actions.declare_file(base_path + "/" + subdir + "/" + header)
        for header in header_files
    ]
    sources = [
        ctx.actions.declare_file(base_path + "/" + subdir + "/" + source)
        for source in source_files
    ]

    ctx.actions.run(
        executable = sdk.fidlgen_cpp,
        arguments = [
            "--json",
            ir.path,
            "--root",
            # get the full path before the subdir
            headers[0].path.rpartition(subdir)[0],
        ],
        inputs = [ir],
        outputs = headers + sources,
        mnemonic = "FidlGenCc",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )

    return [
        _CodegenInfo(files = depset(sources)),
        DefaultInfo(files = depset(headers)),
    ]

# Runs fidlgen to produce both the header file and the implementation file.
# Only exposes the header as a source, as the two files need to be consumed by
# the cc_library as two separate rules.
_codegen = rule(
    implementation = _codegen_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "library": attr.label(
            doc = "The FIDL library to generate code for",
            mandatory = True,
            allow_files = False,
            providers = [FuchsiaFidlLibraryInfo],
        ),
        "binding_type": attr.string(
            doc = "Type of bindings to expose",
            mandatory = True,
        ),
    } | COMPATIBILITY.FUCHSIA_ATTRS,
)

def _impl_wrapper_impl(ctx):
    files = ctx.attr.codegen[_CodegenInfo].files
    return [DefaultInfo(files = files)]

# Simply declares the implementation file generated by the codegen target as an
# output.
# This allows the implementation file to be exposed as a source in its own rule.
_impl_wrapper = rule(
    implementation = _impl_wrapper_impl,
    attrs = {
        "codegen": attr.label(
            doc = "The codegen rules generating the implementation file",
            mandatory = True,
            allow_files = False,
            providers = [_CodegenInfo],
        ),
    },
)
