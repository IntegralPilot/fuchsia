"""Implementation of cc_bind_rules rule"""

load(
    "@fuchsia_rules_common//:utils.bzl",
    "make_resource_struct",
)
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(
    ":providers.bzl",
    "FuchsiaBindLibraryInfo",
    "FuchsiaPackageResourcesInfo",
)
load(":utils.bzl", "wrap_executable")

_COMMON_ATTR = {
    "rules": attr.label(
        doc = "Path to the bind rules source file",
        mandatory = True,
        allow_single_file = True,
    ),
    "deps": attr.label_list(
        doc = "The list of libraries this library depends on",
        mandatory = False,
        providers = [FuchsiaBindLibraryInfo],
    ),
}

def _process_bindc_args(ctx):
    # Collect all the bind files and their filepaths that will be passed to bindc.
    inputs = []
    include_filepaths = []

    for dep in ctx.attr.deps:
        trans_srcs = dep[FuchsiaBindLibraryInfo].transitive_sources
        for src in trans_srcs.to_list():
            # Only add unique instances.
            if src.path in include_filepaths:
                continue
            inputs.append(src)
            if len(include_filepaths) == 0:
                include_filepaths.append("--include")

            include_filepaths.append(src.path)

    return {
        "inputs": inputs,
        "include_filepaths": include_filepaths,
    }

def _fuchsia_driver_bind_bytecode_impl(ctx):
    args = _process_bindc_args(ctx)
    sdk = get_fuchsia_sdk_toolchain(ctx)
    ctx.actions.run(
        executable = sdk.bindc,
        arguments = [
                        "compile",
                    ] + args["include_filepaths"] +
                    [
                        "--output",
                        ctx.outputs.output.path,
                        ctx.file.rules.path,
                    ],
        inputs = args["inputs"] + [ctx.file.rules],
        outputs = [
            ctx.outputs.output,
        ],
        mnemonic = "Bindcbc",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
    )
    return [
        FuchsiaPackageResourcesInfo(
            resources = [make_resource_struct(
                src = ctx.outputs.output,
                dest = "meta/bind/" + ctx.outputs.output.basename,
            )],
        ),
    ]

fuchsia_driver_bind_bytecode = rule(
    implementation = _fuchsia_driver_bind_bytecode_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "output": attr.output(
            mandatory = True,
        ),
    } | COMPATIBILITY.FUCHSIA_ATTRS | _COMMON_ATTR,
)

def _fuchsia_driver_bind_bytecode_test_impl(ctx):
    args = _process_bindc_args(ctx)
    sdk = get_fuchsia_sdk_toolchain(ctx)
    executable, runfiles = wrap_executable(
        ctx,
        sdk.bindc.path,
        "test",
        "--lint",
        ctx.file.rules.path,
        "--test-spec",
        ctx.file.tests.path,
        *args["include_filepaths"]
    )
    return [
        DefaultInfo(
            executable = executable,
            runfiles = ctx.runfiles(
                args["inputs"] + [
                    ctx.file.rules,
                    ctx.file.tests,
                ],
            ).merge(runfiles),
        ),
    ]

fuchsia_driver_bind_bytecode_test = rule(
    implementation = _fuchsia_driver_bind_bytecode_test_impl,
    test = True,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "tests": attr.label(
            doc = "Path to the test_spec file",
            mandatory = True,
            allow_single_file = True,
        ),
    } | COMPATIBILITY.FUCHSIA_ATTRS | _COMMON_ATTR,
)
