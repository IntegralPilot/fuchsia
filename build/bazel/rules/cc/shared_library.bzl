# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Shared library rules for Fuchsia."""

load("@rules_cc//cc:defs.bzl", "cc_import")
load("@rules_cc//cc/common:cc_shared_library_info.bzl", "CcSharedLibraryInfo")
load("//build/bazel/rules:current_platform_info.bzl", "CurrentPlatformInfo")
load("//build/bazel/rules:golden_files.bzl", "verify_golden_files")
load("//build/bazel/rules/cc:providers.bzl", "PrebuiltLibraryInfo")

visibility([
    "//build/bazel/rules/idk/...",
    "//src/zircon/lib/zircon/...",
])

def _to_clang_cpu(cpu):
    if cpu == "arm64":
        return "aarch64"
    elif cpu == "riscv64":
        return "riscv64"
    elif cpu == "x64":
        return "x86_64"
    else:
        fail("Unknown CPU: %s" % cpu)

def _get_stripped_ifs_file_impl(ctx):
    ifs_file = ctx.attr.prebuilt_library[PrebuiltLibraryInfo].stripped_ifs_file
    if not ifs_file:
        fail("`stripped_ifs_file` must not be empty")
    return [DefaultInfo(files = depset([ifs_file]))]

_get_stripped_ifs_file = rule(
    doc = "Simply declares the `stripped_ifs_file` from the `PrebuiltLibraryInfo` " +
          "provider as an output. This allows use of this file in macros.",
    implementation = _get_stripped_ifs_file_impl,
    attrs = {
        "prebuilt_library": attr.label(
            doc = "The prebuilt library target",
            mandatory = True,
            providers = [PrebuiltLibraryInfo],
        ),
    },
)

def _generate_link_stubs_for_shared_library_impl(ctx):
    shared_library = ctx.file.shared_library

    # For convenience, the IFS file name should match the name of the link stub.
    link_stub_name = shared_library.basename
    link_stub_extension = shared_library.extension
    ifs_file_name_base = link_stub_name.removesuffix(link_stub_extension)

    unstripped_ifs_output = ctx.actions.declare_file(ifs_file_name_base + "full_ifs.ifs")
    stripped_ifs_output = ctx.actions.declare_file(ifs_file_name_base + "ifs")
    link_stub_output = ctx.actions.declare_file("link_stub/" + link_stub_name)

    args = ctx.actions.args()
    args.add("--write-if-changed")
    args.add("--output-ifs", unstripped_ifs_output)
    args.add("--output-elf", link_stub_output)
    args.add(shared_library)

    ctx.actions.run(
        inputs = [shared_library],
        outputs = [unstripped_ifs_output, link_stub_output],
        executable = ctx.executable._llvm_ifs,
        arguments = [args],
        mnemonic = "LlvmIfs",
        progress_message = "Generating link stub and IFS file for %{label}",
    )

    strip_args = ctx.actions.args()
    strip_args.add("--strip-undefined")
    strip_args.add("--strip-ifs-target")
    strip_args.add("--strip-needed")
    strip_args.add("--strip-size")
    strip_args.add(unstripped_ifs_output)
    strip_args.add("--output-ifs", stripped_ifs_output)

    ctx.actions.run(
        inputs = [unstripped_ifs_output],
        outputs = [stripped_ifs_output],
        executable = ctx.executable._llvm_ifs,
        arguments = [strip_args],
        mnemonic = "LlvmIfsStrip",
        progress_message = "Stripping IFS file for %{label}",
    )

    return [
        DefaultInfo(files = depset([
            shared_library,
            link_stub_output,
            unstripped_ifs_output,
            stripped_ifs_output,
        ])),
        PrebuiltLibraryInfo(
            type = "shared",
            debug = shared_library,
            link_lib = link_stub_output,
            unstripped_ifs_file = unstripped_ifs_output,
            stripped_ifs_file = stripped_ifs_output,
        ),
    ]

generate_link_stubs_for_shared_library = rule(
    doc = "Generates link stub `.so` and `.ifs` files for a shared library.",
    implementation = _generate_link_stubs_for_shared_library_impl,
    attrs = {
        "shared_library": attr.label(
            doc = "The shared library file for which to create link stubs.",
            allow_single_file = True,
            providers = [CcSharedLibraryInfo],
            mandatory = True,
        ),
        "_llvm_ifs": attr.label(
            default = "@prebuilt_clang//:bin/llvm-ifs",
            executable = True,
            allow_single_file = True,
            cfg = "exec",
        ),
    },
)

def _merge_library_info_for_shared_library_impl(ctx):
    prebuilt_library_info = ctx.attr.link_stub_info[PrebuiltLibraryInfo]

    if prebuilt_library_info.type != "shared":
        fail("`link_stub_info` must be a shared library.")
    if hasattr(prebuilt_library_info, "stripped"):
        fail("`link_stub_info` must not already contain `stripped`.")

    return [
        DefaultInfo(files = depset(transitive = [
            ctx.attr.link_stub_info[DefaultInfo].files,
            ctx.attr.stripped_binary[DefaultInfo].files,
        ])),
        # LINT.IfChange(prebuilt_library_info)
        PrebuiltLibraryInfo(
            type = prebuilt_library_info.type,
            debug = prebuilt_library_info.debug,
            stripped = ctx.attr.stripped_binary[DefaultInfo].files.to_list()[0],
            link_lib = prebuilt_library_info.link_lib,
            unstripped_ifs_file = prebuilt_library_info.unstripped_ifs_file,
            stripped_ifs_file = prebuilt_library_info.stripped_ifs_file,
        ),
        # LINT.ThenChange(//build/bazel/rules/cc/providers.bzl:prebuilt_library_info)
    ]

_merge_library_info_for_shared_library = rule(
    doc = "Returns a fully populated `PrebuiltLibraryInfo` for a shared library.",
    implementation = _merge_library_info_for_shared_library_impl,
    attrs = {
        "link_stub_info": attr.label(
            doc = "A target containing `PrebuiltLibraryInfo` for the shared library and its stubs.",
            providers = [PrebuiltLibraryInfo],
            mandatory = True,
        ),
        "stripped_binary": attr.label(
            doc = "The stripped binary for the shared library.",
            allow_single_file = True,
            mandatory = True,
        ),
    },
)

def _generate_companion_files_for_shared_library_impl(
        name,
        shared_library,
        testonly,
        visibility):
    link_stubs_target_name = name + ".generate_link_stubs"
    generate_link_stubs_for_shared_library(
        name = link_stubs_target_name,
        shared_library = shared_library,
        testonly = testonly,
    )

    _merge_library_info_for_shared_library(
        name = name,
        link_stub_info = link_stubs_target_name,
        # TODO(https://fxbug.dev/443982549): Use the stripped binary once it is available.
        stripped_binary = shared_library,
        testonly = testonly,
        visibility = visibility,
    )

generate_companion_files_for_shared_library = macro(
    doc = "Generates a link stub, IFS file, and stripped binary for a shared library.",
    implementation = _generate_companion_files_for_shared_library_impl,
    attrs = {
        "shared_library": attr.label(
            doc = "The shared library file for which to create companion files.",
            allow_single_file = True,
            providers = [CcSharedLibraryInfo],
            mandatory = True,
            configurable = False,
        ),
        "testonly": attr.bool(
            doc = "Standard meaning.",
            default = False,
            configurable = False,
        ),
    },
)

def _get_library_info_for_static_library_impl(ctx):
    static_library = ctx.file.static_library
    return [
        DefaultInfo(files = depset([static_library])),
        PrebuiltLibraryInfo(
            type = "static",
            link_lib = static_library,
        ),
    ]

get_library_info_for_static_library = rule(
    doc = "Returns `PrebuiltLibraryInfo` for a static library." +
          "This is useful when a rule supports passing both static and shared libraries.",
    implementation = _get_library_info_for_static_library_impl,
    attrs = {
        "static_library": attr.label(
            doc = "The static library file for which to create library info.",
            allow_single_file = True,
            mandatory = True,
        ),
    },
)

def _verify_public_symbols_impl(
        name,
        prebuilt_library,
        reference,
        library_name,
        testonly,
        visibility):
    ifs_file_target_name = name + ".currentifs_file"

    _get_stripped_ifs_file(
        name = ifs_file_target_name,
        prebuilt_library = prebuilt_library,
        testonly = testonly,
        visibility = ["//visibility:private"],
    )

    verify_golden_files(
        name = name,
        candidate_files = [ifs_file_target_name],
        golden_file = reference,
        message = "ABI has changed! In library {}".format(library_name),
        testonly = testonly,
        visibility = visibility,
    )

verify_public_symbols = macro(
    doc = """Verifies the list of public symbols from a prebuilt library against a golden file.

When adding a new library to be tracked by this macro, you can create an empty
file (usually named `[lib]${library_name}.ifs`), run the build, then run the
`cp` command suggested by the build failure message to populate the golden file.""",
    implementation = _verify_public_symbols_impl,
    attrs = {
        "prebuilt_library": attr.label(
            doc = "The prebuilt library target",
            mandatory = True,
            providers = [PrebuiltLibraryInfo],
        ),
        "reference": attr.label(
            doc = "The checked-in reference IFS file.",
            mandatory = True,
            allow_single_file = True,
        ),
        "library_name": attr.string(
            doc = "A human-readable library name for debugging purposes.",
            mandatory = True,
            configurable = False,
        ),
        "testonly": attr.bool(
            doc = "Usual Bazel meaning.",
            default = False,
            configurable = False,
        ),
    },
)

def _link_stub_from_ifs_file_impl(ctx):
    ifs_file = ctx.file.ifs_file
    link_stub_file_name = ctx.attr.stub_name + ".so" if ctx.attr.stub_name else (
        ifs_file.basename.removesuffix(ifs_file.extension) + "so"
    )

    link_stub_output = ctx.actions.declare_file(link_stub_file_name)

    current_cpu = ctx.attr._current_platform[CurrentPlatformInfo].cpu
    clang_cpu = _to_clang_cpu(current_cpu)

    args = ctx.actions.args()
    args.add("--input-format=IFS")
    args.add("--target=%s-fuchsia" % clang_cpu)
    args.add("--write-if-changed")
    args.add("--output-elf", link_stub_output)
    args.add(ifs_file)

    ctx.actions.run(
        inputs = [ifs_file],
        outputs = [link_stub_output],
        executable = ctx.executable._llvm_ifs,
        arguments = [args],
        mnemonic = "LlvmIfs",
        progress_message = "Generating link stub for %{label}",
    )

    return [DefaultInfo(files = depset([link_stub_output]))]

link_stub_from_ifs_file = rule(
    doc = "Generates link stub `.so` file from an `.ifs` file." +
          "The stub file will have the same base name as the `.ifs` file." +
          "The file is not usable with Bazel cc rules/macros." +
          "For that, use cc_link_stub_from_ifs_file().",
    implementation = _link_stub_from_ifs_file_impl,
    attrs = {
        "ifs_file": attr.label(
            doc = "The `.ifs` file from which to create the link stub.",
            allow_single_file = True,
            mandatory = True,
        ),
        "stub_name": attr.string(
            doc = "The base name of the stub file. `.so` will be appended." +
                  "If not specified, the base name of the `.ifs` file will be used.",
            mandatory = False,
        ),
        "_llvm_ifs": attr.label(
            default = "@prebuilt_clang//:bin/llvm-ifs",
            executable = True,
            allow_single_file = True,
            cfg = "exec",
        ),
        "_current_platform": attr.label(
            providers = [CurrentPlatformInfo],
            default = "@//build/bazel:current_platform",
        ),
    },
)

def _cc_link_stub_from_ifs_file_impl(
        name,
        deps,
        system_provided,
        target_compatible_with,
        testonly,
        visibility,
        **kwargs):
    stub_rule_name = name + ".link_stub"
    link_stub_from_ifs_file(
        name = stub_rule_name,
        target_compatible_with = target_compatible_with,
        testonly = testonly,
        visibility = visibility,
        **kwargs
    )

    cc_import(
        name = name,
        deps = deps,
        shared_library = stub_rule_name if not system_provided else None,
        interface_library = stub_rule_name if system_provided else None,
        system_provided = system_provided,
        target_compatible_with = target_compatible_with,
        testonly = testonly,
        visibility = visibility,
    )

cc_link_stub_from_ifs_file = macro(
    doc = "Generates link stub `.so` file from an `.ifs` file and imports it for use in cc rules." +
          "The stub file will have the same base name as the `.ifs` file.",
    inherit_attrs = link_stub_from_ifs_file,
    implementation = _cc_link_stub_from_ifs_file_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "Standard meaning.",
            default = [],
        ),
        "system_provided": attr.bool(
            doc = "See `cc_import()`.",
            default = False,
        ),
        "target_compatible_with": attr.label_list(
            doc = "Standard meaning.",
            default = [],
        ),
    },
)
