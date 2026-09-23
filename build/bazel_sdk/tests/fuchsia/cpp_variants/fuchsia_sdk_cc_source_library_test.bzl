# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

""" A test which verifies that c++ libraries can be compiled using different
versions of the cpp toolchain. """

load("@fuchsia_sdk//:generated_constants.bzl", "ALL_CC_SOURCE_TARGETS")
load(
    "@rules_fuchsia//fuchsia/private:fuchsia_api_level.bzl",
    "FUCHSIA_API_LEVEL_TARGET",
)

NATIVE_CPU_ALIASES = {
    "darwin": "x86_64",
    "k8": "x86_64",
    "x86_64": "x86_64",
    "armeabi-v7a": "aarch64",
    "aarch64": "aarch64",
    "darwin_arm64": "aarch64",
    "darwin_x86_64": "x86_64",
    "riscv64": "riscv64",
}

FUCHSIA_PLATFORMS_MAP = {
    "x86_64": "fuchsia_x64",
    "aarch64": "fuchsia_arm64",
    "riscv64": "fuchsia_riscv64",
}

# Build at HEAD to hopefully exercise as much code as possible.
# The downside is that code removed at HEAD (or NEXT or a recent stable API
# level) will not be built.
_TARGET_API_LEVEL = "HEAD"
_TARGET_API_LEVEL_AS_U32 = 4292870144  # u32_for_fuchsia_api_level(_TARGET_API_LEVEL)

def _cc_std_version_transition_impl(settings, attr):
    input_cpu = settings["//command_line_option:cpu"]
    output_cpu = NATIVE_CPU_ALIASES.get(input_cpu, None)

    if not output_cpu:
        fail("Unrecognized cpu %s." % input_cpu)
    fuchsia_platform = "@rules_fuchsia//fuchsia/constraints/platforms:" + FUCHSIA_PLATFORMS_MAP[output_cpu]

    cxxopts = settings["//command_line_option:cxxopt"] + [
        "-std={}".format(attr.cc_version),
    ] + attr.extra_cxxopts

    copts = settings["//command_line_option:copt"] + [
        "-ffuchsia-api-level=%s" % _TARGET_API_LEVEL_AS_U32,
    ] + attr.extra_copts

    return {
        "//command_line_option:cpu": output_cpu,
        "//command_line_option:cxxopt": cxxopts,
        "//command_line_option:copt": copts,
        "//command_line_option:platforms": fuchsia_platform,
        FUCHSIA_API_LEVEL_TARGET: _TARGET_API_LEVEL,
    }

# This transition replicates some of the functionality of `fuchsia_transition`,
# including setting the API level label and adding it to the compiler args.
cc_std_version_transition = transition(
    implementation = _cc_std_version_transition_impl,
    inputs = [
        "//command_line_option:cpu",
        "//command_line_option:cxxopt",
        "//command_line_option:copt",
    ],
    outputs = [
        "//command_line_option:cpu",
        "//command_line_option:cxxopt",
        "//command_line_option:platforms",
        "//command_line_option:copt",
        FUCHSIA_API_LEVEL_TARGET,
    ],
)

# buildifier: disable=function-docstring
def fuchsia_sdk_cc_source_library_test(name, cc_version, copts = [], ignored_deps = [], **kwargs):
    driver_binary_name = name + "_variant_test"
    driver_src_name = driver_binary_name + ".cc"

    allowed_deps = {}
    for k, v in ALL_CC_SOURCE_TARGETS.items():
        if k not in ignored_deps:
            allowed_deps["@fuchsia_sdk" + k] = v

    _gen_cpp_variants_test_driver(
        name = name + "_driver",
        allowed_deps = allowed_deps,
        source_file = driver_src_name,
    )

    # Note: we cannot simply add the copts to this library because copts propagate
    # in the opposite direction. This means that we will not pass them to the sdk
    # libraries which is what we are trying to achieve.
    native.cc_binary(
        name = driver_binary_name,
        srcs = [driver_src_name],
        # Pass along the c++ SDK deps with any ignored_deps removed.
        deps = allowed_deps,
    )

    _fuchsia_sdk_cc_source_library_test(
        name = name,
        driver_binary = driver_binary_name,
        cc_version = cc_version,
        extra_copts = copts,
        **kwargs
    )

def _gen_cpp_variants_test_driver_impl(ctx):
    includes_str = ""
    for _, includes in ctx.attr.allowed_deps.items():
        for include in includes:
            includes_str += '#include "%s"\n' % include

    ctx.actions.expand_template(
        template = ctx.file._template,
        output = ctx.outputs.source_file,
        substitutions = {
            "{INCLUDES}": includes_str,
        },
    )

_gen_cpp_variants_test_driver = rule(
    implementation = _gen_cpp_variants_test_driver_impl,
    attrs = {
        "allowed_deps": attr.string_list_dict(
            default = {},
        ),
        "_template": attr.label(
            default = "variant_test.tmpl",
            allow_single_file = True,
        ),
        "source_file": attr.output(
            mandatory = True,
        ),
    },
)

def _fuchsia_sdk_cc_source_library_test_impl(ctx):
    # Create a test script which depends on the driver_binary
    # to ensure it gets built and can be executed directly by bazel test.
    script = ctx.actions.declare_file("ensure_built.sh")

    command = 'echo "echo %s\n" > %s\n' % (ctx.executable.driver_binary.short_path, script.path)
    ctx.actions.run_shell(
        outputs = [script],
        tools = [ctx.executable.driver_binary],
        command = command,
    )

    return [
        DefaultInfo(
            executable = script,
            runfiles = ctx.runfiles(
                files = [ctx.files.driver_binary[0]],
            ),
        ),
    ]

_fuchsia_sdk_cc_source_library_test = rule(
    test = True,
    implementation = _fuchsia_sdk_cc_source_library_test_impl,
    # Use a custom transition to control the exact compiler options.
    cfg = cc_std_version_transition,
    attrs = {
        "driver_binary": attr.label(
            mandatory = True,
            allow_single_file = True,
            executable = True,
            cfg = cc_std_version_transition,
        ),
        "cc_version": attr.string(
            mandatory = True,
            values = ["c++17", "c++20"],
        ),
        "extra_copts": attr.string_list(
            default = [],
        ),
        "extra_cxxopts": attr.string_list(
            default = [],
        ),
        "ignored_deps": attr.string_list(
            default = [],
        ),
        "_allowlist_function_transition": attr.label(
            default = "@bazel_tools//tools/allowlists/function_transition_allowlist",
        ),
    },
)
