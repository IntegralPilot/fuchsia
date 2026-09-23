# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A rust_library backed by a FIDL library."""

load("@rules_rust//rust:defs.bzl", "rust_library")

visibility("private")

def fidl_rust_library(
        *,
        name,
        fidl_library_name,
        fidl_ir_json,
        deps,
        contains_drivers,
        testonly,
        visibility):
    """
    Generates a `rust_library()` providing the generated Rust bindings for a given FIDL library.

    Args:
        name: String base name of the `rust_library()` target.
        fidl_library_name: String name of the FIDL library for which bindings are generated.
        fidl_ir_json: `Label` pointing to a single file containing the FIDL IR
            representation of the `fidl_library_name` library.
        deps: List of `Label`s for FIDL libraries that the `fidl_library_name` library depends on.
        contains_drivers: Boolean indicating whether the `fidl_library_name`
            library supports drivers.
        testonly: usual meaning.
        visibility: usual meaning.
    """

    _fidl_rust_library_flavor("fidl", name, fidl_library_name, fidl_ir_json, deps, contains_drivers, testonly, visibility)
    _fidl_rust_library_flavor("common", name, fidl_library_name, fidl_ir_json, deps, contains_drivers, testonly, ["//visibility:private"])
    _fidl_rust_library_flavor("fdomain", name, fidl_library_name, fidl_ir_json, deps, contains_drivers, testonly, visibility)
    _fidl_rust_library_flex("fidl", name, fidl_library_name, testonly, visibility)
    _fidl_rust_library_flex("fdomain", name, fidl_library_name, testonly, visibility)

def _fidl_rust_flavor_crate_name(fidl_library_name, flavor):
    base = fidl_library_name.replace(".", "_")
    if flavor == "fidl":
        return "fidl_%s" % base
    elif flavor == "common":
        return "fidl_%s_common" % base
    elif flavor == "fdomain":
        return "fdomain_%s" % base
    else:
        fail("Unknown flavor: %s" % flavor)

def _fidl_rust_flavor_file_name(name, flavor):
    base = name.replace(".", "_")
    if flavor == "fidl":
        return "%s.rs" % base
    elif flavor == "common":
        return "%s_common.rs" % base
    elif flavor == "fdomain":
        return "%s__fdomain.rs" % base
    else:
        fail("Unknown flavor: %s" % flavor)

def _fidl_rust_flavor_label_suffix(flavor):
    if flavor == "fidl":
        return "rust"
    elif flavor == "common":
        return "rust_common"
    elif flavor == "fdomain":
        return "rust_fdomain"
    else:
        fail("Unknown flavor: %s" % flavor)

def _fidl_rust_flavor_label_name(label_name, flavor):
    return label_name + "_" + _fidl_rust_flavor_label_suffix(flavor)

def _fidl_rust_flavor_label(label, flavor):
    return "//%s:%s" % (label.package, _fidl_rust_flavor_label_name(label.name, flavor))

def _fidl_rust_library_flavor(flavor, name, fidl_library_name, fidl_ir_json, deps, contains_drivers, testonly, visibility):
    flavor_label = name + "_" + _fidl_rust_flavor_label_suffix(flavor)

    fidlgen_label = flavor_label + "_fidlgen"

    if flavor == "common":
        use_common = ""
    else:
        use_common = _fidl_rust_flavor_crate_name(fidl_library_name, "common")

    _fidlgen_rust(
        name = fidlgen_label,
        fidl_ir_json = fidl_ir_json,
        out = "bindings/rust/%s.rs" % _fidl_rust_flavor_file_name(name, flavor),
        contains_drivers = contains_drivers,
        common = (flavor == "common"),
        fdomain = (flavor == "fdomain"),
        use_common = use_common,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    library_deps = [
        "//sdk/rust/zx-status",
        "//src/lib/fidl/rust/fidl",
        "//third_party/rust_crates/vendor:bitflags",
        "//third_party/rust_crates/vendor:futures",
    ]

    if flavor == "fdomain":
        library_deps.append("//src/lib/fdomain/client")

    for dep in deps:
        if Label(dep) == Label("//zircon/vdso/zx"):
            continue

        library_deps.append(_fidl_rust_flavor_label(dep, flavor))

    if flavor != "common":
        library_deps.append(":" + _fidl_rust_flavor_label_name(fidl_library_name, "common"))

    if contains_drivers:
        # TODO(https://fxbug.dev/503359085): driver transport support
        #     library_deps += [
        #         "//src/lib/fidl/rust/fidl_driver",
        #         "//sdk/lib/driver/runtime/rust",
        #     ]
        pass

    # `select()` must be appended last.
    library_deps += select({
        "@platforms//os:fuchsia": ["//sdk/rust/zx"],
        "//conditions:default": [],
    })

    rust_library(
        name = flavor_label,
        crate_name = _fidl_rust_flavor_crate_name(fidl_library_name, flavor),
        srcs = [fidlgen_label],
        deps = library_deps,
        edition = "2018",
        tags = ["noclippy"],
        testonly = testonly,
        visibility = visibility,
    )

def _fidl_rust_library_flex(flavor, name, fidl_library_name, testonly, visibility):
    original_label = name + "_" + _fidl_rust_flavor_label_suffix(flavor)
    original_crate_name = _fidl_rust_flavor_crate_name(fidl_library_name, flavor)

    flex_label = original_label + "_flex"
    flex_generate_label = flex_label + "_generate"

    base_library_name = fidl_library_name.replace(".", "_")
    flex_crate_name = "flex_%s" % base_library_name

    flex_file_name = "bindings/rust/%s_%s_flex.rs" % (name.replace(".", "_"), flavor)

    native.genrule(
        name = flex_generate_label,
        outs = [flex_file_name],
        cmd = "echo 'pub use %s::*;' > $@" % original_crate_name,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    rust_library(
        name = flex_label,
        crate_name = flex_crate_name,
        srcs = [flex_generate_label],
        deps = [":" + original_label],
        edition = "2024",
        tags = ["noclippy"],
        testonly = testonly,
        visibility = visibility,
    )

def _fidlgen_rust_impl(ctx):
    if ctx.attr.common != (ctx.attr.use_common == ""):
        fail("'use_common' must be empty if and only if `common` is True.")

    ir = ctx.file.fidl_ir_json

    rust_toolchain = ctx.toolchains["@rules_rust//rust:toolchain_type"]
    rustfmt = rust_toolchain.rustfmt

    arguments = [
        "--json",
        ir.path,
        "--output-filename",
        ctx.outputs.out.path,
        "--rustfmt",
        rustfmt.path,
    ]

    if ctx.attr.contains_drivers:
        arguments.append("--include-drivers")
    if ctx.attr.common:
        arguments.append("--common")
    if ctx.attr.fdomain:
        arguments.append("--fdomain")
    if ctx.attr.use_common != "":
        arguments.append("--use_common=" + ctx.attr.use_common)

    ctx.actions.run(
        executable = ctx.executable._fidlgen_tool,
        arguments = arguments,
        inputs = [ir, rustfmt],
        tools = rust_toolchain.all_files,
        outputs = [ctx.outputs.out],
        mnemonic = "FidlGenRust",
    )

    return [
        DefaultInfo(files = depset([ctx.outputs.out])),
    ]

_fidlgen_rust = rule(
    implementation = _fidlgen_rust_impl,
    toolchains = ["@rules_rust//rust:toolchain_type"],
    attrs = {
        "fidl_ir_json": attr.label(
            doc = "The FIDL IR for which to generate code.",
            allow_single_file = True,
            mandatory = True,
        ),
        "contains_drivers": attr.bool(
            doc = "Indicates if any of the FIDL files contain the driver transport or " +
                  "references to the driver transport.",
            mandatory = True,
        ),
        "common": attr.bool(
            doc = "If `True`, generate only the common (non-resource) data structures.",
            default = False,
        ),
        "fdomain": attr.bool(
            doc = "If `True`, generate FDomain bindings.",
            default = False,
        ),
        "use_common": attr.string(
            doc = "If not empty, use the given crate name for the common (non-resource) data structures.",
            default = "",
        ),
        "_fidlgen_tool": attr.label(
            doc = "fidlgen_rust tool.",
            executable = True,
            cfg = "exec",
            default = "@//tools/fidl/fidlgen_rust",
        ),
        "out": attr.output(
            doc = "Output filename.",
            mandatory = True,
        ),
    },
)
