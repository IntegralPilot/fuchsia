# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Implement fuchsia_prebuilt_package() rule."""

load("@bazel_skylib//rules:select_file.bzl", "select_file")
load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load("@fuchsia_rules_common//debug_symbols:providers.bzl", "FuchsiaDebugSymbolInfo")
load("@fuchsia_rules_common//packages:prebuilt_package.bzl", "unpack_prebuilt_package_impl")
load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load("//fuchsia/private/workflows:fuchsia_package_tasks.bzl", "fuchsia_package_tasks")
load(":providers.bzl", "FuchsiaComponentInfo", "FuchsiaPackagedComponentInfo")

def _component_basename(cm_str):
    return cm_str.rpartition("/")[-1].removesuffix(".cm")

def _make_component_info(ctx):
    return [
        FuchsiaPackagedComponentInfo(
            dest = c,
            component_info = FuchsiaComponentInfo(
                name = _component_basename(c),
                is_driver = False,
                is_test = False,
                run_tag = _component_basename(c),
            ),
        )
        for c in ctx.attr.components
    ] + [
        FuchsiaPackagedComponentInfo(
            dest = d,
            component_info = FuchsiaComponentInfo(
                name = _component_basename(d),
                is_driver = True,
                is_test = False,
                run_tag = _component_basename(d),
            ),
        )
        for d in ctx.attr.drivers
    ] + [
        FuchsiaPackagedComponentInfo(
            dest = t,
            component_info = FuchsiaComponentInfo(
                name = _component_basename(t),
                is_driver = False,
                is_test = True,
                run_tag = _component_basename(t),
            ),
        )
        for t in ctx.attr.test_components
    ]

def _unpack_prebuilt_package_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    return unpack_prebuilt_package_impl(
        ctx,
        package_tool = sdk.ffx_package,
        package_tool_is_ffx = True,
        packaged_components = _make_component_info(ctx),
    )

_unpack_prebuilt_package = rule(
    doc = """Provides access to a fuchsia package from a prebuilt package archive (.far).""",
    implementation = _unpack_prebuilt_package_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "archive": attr.label(
            doc = "The fuchsia archive (typically a .far file).",
            allow_single_file = True,
            mandatory = True,
        ),
        "components": attr.string_list(
            doc = "ordinary components in this package",
            default = [],
        ),
        "drivers": attr.string_list(
            doc = "driver components in this package",
            default = [],
        ),
        "test_components": attr.string_list(
            doc = "test components in this package",
            default = [],
        ),
        "_rebase_package_manifest": attr.label(
            default = "@fuchsia_rules_common//packages:rebase_package_manifest",
            executable = True,
            cfg = "exec",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def _pack_prebuilt_package_impl(ctx):
    sdk = get_fuchsia_sdk_toolchain(ctx)

    # Inputs
    manifest = ctx.files.manifest[0]
    input_files = ctx.files.files

    # Outputs
    far_file = ctx.actions.declare_file("%s.far" % ctx.attr.name)
    output_files = [far_file, manifest]

    # An environment variable that creates an isolated FFX instance.
    ffx_isolate_dir = ctx.actions.declare_directory(ctx.label.name + "_pkg/_package.ffx")

    # Create the far file.
    ctx.actions.run(
        executable = sdk.ffx_package,
        arguments = [
            "--isolate-dir",
            ffx_isolate_dir.path,
            "package",
            "archive",
            "create",
            manifest.path,
            "-o",
            far_file.path,
        ],
        inputs = input_files,
        outputs = [far_file, ffx_isolate_dir],
        mnemonic = "FuchsiaFfxPackageArchiveCreate",
        progress_message = "Archiving package for %{label}",
        toolchain = FUCHSIA_TOOLCHAIN_DEFINITION,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    return [
        DefaultInfo(files = depset(output_files)),
        FuchsiaPackageInfo(
            package_manifest = manifest,
            far_file = far_file,
            packaged_components = _make_component_info(ctx),
            files = output_files + input_files,
            build_id_dirs = [],
        ),
        # TODO(https://fxbug.dev/338180287): Add debug symbols support.
        FuchsiaDebugSymbolInfo(build_id_dirs_mapping = {}),
    ]

_pack_prebuilt_package = rule(
    doc = """Provides access to a fuchsia package from a package manifest.""",
    implementation = _pack_prebuilt_package_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "manifest": attr.label(
            doc = "The package's manifest file",
            allow_single_file = True,
            mandatory = True,
        ),
        "files": attr.label_list(
            doc = "Files that are part of the package.",
            allow_files = True,
            mandatory = True,
        ),
        "components": attr.string_list(
            doc = "ordinary components in this package",
            default = [],
        ),
        "drivers": attr.string_list(
            doc = "driver components in this package",
            default = [],
        ),
        "test_components": attr.string_list(
            doc = "test components in this package",
            default = [],
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)

def _make_prebuilt_package(
        *,
        name,
        archive,
        manifest,
        files,
        components = [],
        drivers = [],
        test_components = [],
        **kwargs):
    if (archive and files) or bool(archive) == bool(manifest):
        fail("Must specify exactly either `archive` or `manifest + files`.")
    if archive:
        _unpack_prebuilt_package(
            name = "%s_fuchsia_package" % name,
            archive = archive,
            drivers = drivers,
            components = components,
            test_components = test_components,
            **kwargs
        )
    else:
        _pack_prebuilt_package(
            name = "%s_fuchsia_package" % name,
            manifest = manifest,
            files = files,
            drivers = drivers,
            components = components,
            test_components = test_components,
            **kwargs
        )

        select_file(
            name = name + ".far",
            srcs = ":%s_fuchsia_package" % name,
            subpath = "%s_fuchsia_package.far" % name,
            **kwargs
        )

# buildifier: disable=function-docstring
def fuchsia_prebuilt_package(
        *,
        name,
        archive = None,
        manifest = None,
        files = [],
        components = [],
        drivers = [],
        **kwargs):
    _make_prebuilt_package(
        name = name,
        archive = archive,
        manifest = manifest,
        files = files,
        components = components,
        drivers = drivers,
        **kwargs
    )

    fuchsia_package_tasks(
        name = name,
        package = "%s_fuchsia_package" % name,
        component_run_tags = [_component_basename(c) for c in components + drivers],
        **kwargs
    )

# buildifier: disable=function-docstring
def fuchsia_prebuilt_test_package(
        *,
        name,
        archive = None,
        manifest = None,
        files = [],
        test_components = [],
        test_realm = None,
        enumerated_component_filter = None,
        retries = None,
        disable_retries_on_failure = None,
        **kwargs):
    _make_prebuilt_package(
        name = name,
        archive = archive,
        manifest = manifest,
        files = files,
        test_components = test_components,
        testonly = True,
        **kwargs
    )

    fuchsia_package_tasks(
        name = name,
        package = "%s_fuchsia_package" % name,
        component_run_tags = [_component_basename(c) for c in test_components],
        is_test = True,
        enumerate_test_components = True,
        enumerated_component_filter = enumerated_component_filter,
        retries = retries,
        disable_retries_on_failure = disable_retries_on_failure,
        test_realm = test_realm,
        testonly = True,
        **kwargs
    )
