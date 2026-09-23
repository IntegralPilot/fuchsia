# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common implementation for building fuchsia packages."""

load("@fuchsia_rules_common//:local_actions.bzl", "LOCAL_ONLY_ACTION_KWARGS")
load(
    "@fuchsia_rules_common//:utils.bzl",
    "fuchsia_cpu_from_ctx",
    "make_resource_struct",
    "stub_executable",
)
load(
    "@fuchsia_rules_common//components:providers.bzl",
    "FuchsiaComponentInfo",
    "FuchsiaPackagedComponentInfo",
)
load(
    "@fuchsia_rules_common//debug_symbols:providers.bzl",
    "FuchsiaDebugSymbolInfo",
)
load(
    "@fuchsia_rules_common//packages:providers.bzl",
    "FuchsiaCollectedPackageResourcesInfo",
    "FuchsiaDriverToolInfo",
    "FuchsiaPackageInfo",
    "FuchsiaPackageResourcesInfo",
    "FuchsiaStructuredConfigInfo",
)

COMMON_BUILD_FUCHSIA_PACKAGE_ATTRIBUTES = {
    "package_name": attr.string(
        doc = "The name of the package",
        mandatory = True,
    ),
    "archive_name": attr.string(
        doc = """The name of the generated archive file.

        The extension `.far` will be appended if the name does not already end in this.
        Defaults to `package_name`""",
    ),
    "package_repository_name": attr.string(
        doc = "Repository name of this package, defaults to None",
    ),
    "components": attr.label_list(
        doc = "The list of components included in this package",
        providers = [FuchsiaComponentInfo],
    ),
    "test_components": attr.label_list(
        doc = "The list of test components included in this package",
        providers = [FuchsiaComponentInfo],
    ),
    "resources": attr.label_list(
        doc = "The list of resources included in this package",
        providers = [FuchsiaPackageResourcesInfo],
    ),
    "processed_binaries": attr.label(
        doc = "Label to a find_and_process_unstripped_binaries() target for this package.",
        providers = [FuchsiaPackageResourcesInfo, FuchsiaDebugSymbolInfo],
    ),
    "collected_resources": attr.label(
        doc = "Label to a fuchsia_find_all_package_resources() target for this package.",
        providers = [FuchsiaCollectedPackageResourcesInfo],
        mandatory = True,
    ),
    "tools": attr.label_list(
        doc = "The list of tools included in this package",
        providers = [FuchsiaDriverToolInfo],
    ),
    "subpackages": attr.label_list(
        doc = "The list of subpackages included in this package",
        providers = [FuchsiaPackageInfo],
    ),
    "platform": attr.string(
        doc = """The Fuchsia platform to build for.

        If this value is not set we will fall back to the cpu setting to determine
        the correct platform.
        """,
    ),
    "_validate_component_manifests": attr.label(
        doc = "Tool to validate binary paths in component manifests.",
        default = "@fuchsia_rules_common//packages/tools:validate_component_manifests",
        executable = True,
        cfg = "exec",
    ),
}

def common_build_fuchsia_package_impl(
        ctx,
        ffx_package,
        ffx_package_is_ffx,
        cmc_tool,
        fuchsia_debug_symbol_info,
        api_level = ""):
    """Common implementation for building fuchsia packages.

    This function generates actions to build and archive a Fuchsia package
    containing components, resources, tools, and subpackages.

    The function will declare files for the package manifest json, meta/package
    descriptor, meta.far file, and the final package archive (.far).

    Args:
        ctx: The rule context.
        ffx_package: The ffx package or package-tool executable.
        ffx_package_is_ffx: A boolean. If True, creates FFX isolated
          directories and uses the ffx CLI interface.
        cmc_tool: The component manifest compiler (cmc) tool executable.
        meta_content_append_tool: Tool to append subpackage contents to
          manifests.
        fuchsia_debug_symbol_info: Pre-merged FuchsiaDebugSymbolInfo provider
          for the package.
        api_level: Optionally specify the target API level.

    Returns:
        A list of providers including DefaultInfo, FuchsiaPackageInfo,
        FuchsiaDebugSymbolInfo, and OutputGroupInfo.
    """
    archive_name = ctx.attr.archive_name or ctx.attr.package_name

    if not archive_name.endswith(".far"):
        archive_name += ".far"

    # where we will collect all of the temporary files
    pkg_dir = ctx.label.name + "_pkg/"

    # Declare all of the output files
    manifest = ctx.actions.declare_file(pkg_dir + "manifest")
    meta_package = ctx.actions.declare_file(pkg_dir + "meta/package")
    meta_far = ctx.actions.declare_file(pkg_dir + "meta.far")
    output_package_manifest = ctx.actions.declare_file(pkg_dir + "package_manifest.json")
    far_file = ctx.actions.declare_file(archive_name)

    # All of the resources that will go into the package
    package_resources = [
        # Initially include the meta package
        make_resource_struct(
            src = meta_package,
            dest = "meta/package",
        ),
    ]

    # Add all of the collected resources
    package_resources.extend(
        ctx.attr.collected_resources[FuchsiaCollectedPackageResourcesInfo].collected_resources.to_list(),
    )

    packaged_components = []

    # Verify correctness of test vs non-test components.
    for test_component in ctx.attr.test_components:
        if not test_component[FuchsiaComponentInfo].is_test:
            fail("Please use `components` for non-test components.")
    for component in ctx.attr.components:
        if component[FuchsiaComponentInfo].is_test:
            fail("Please use `test_components` for test components.")

    # Collect all the resources from the deps
    # TODO(342560609) Move all resource publishing from components into the
    # component rules so they get collected into the collected_resources attr.
    for dep in ctx.attr.test_components + ctx.attr.components:
        if FuchsiaStructuredConfigInfo in dep:
            sc_info = dep[FuchsiaStructuredConfigInfo]
            package_resources.append(
                # add the CVF file
                make_resource_struct(
                    src = sc_info.cvf_source,
                    dest = sc_info.cvf_dest,
                ),
            )

        if FuchsiaComponentInfo in dep:
            component_info = dep[FuchsiaComponentInfo]
            component_dest = "meta/%s.cm" % (component_info.name)

            packaged_components.append(FuchsiaPackagedComponentInfo(
                component_info = component_info,
                dest = component_dest,
            ))
        else:
            fail("Unknown dependency type being added to package: %s" % dep.label)

    # Add the resources for stripped ELF binaries.
    if ctx.attr.processed_binaries:
        package_resources.extend(ctx.attr.processed_binaries[FuchsiaPackageResourcesInfo].resources)

    # Write our package_manifest file. Sort for determinism.
    content = "\n".join(["%s=%s" % (r.dest, r.src.path) for r in sorted(package_resources, key = lambda s: s.dest)])

    ctx.actions.write(
        output = manifest,
        content = content,
    )

    # Create the meta/package file
    ctx.actions.write(
        meta_package,
        content = json.encode_indent({
            "name": ctx.attr.package_name,
            "version": "0",
        }),
    )

    build_inputs = [r.src for r in package_resources] + [
        manifest,
        meta_package,
    ]

    repo_name_args = []
    if ctx.attr.package_repository_name:
        repo_name_args = ["--repository", ctx.attr.package_repository_name]

    subpackages_args = []
    subpackages_inputs = []
    subpackages = ctx.attr.subpackages
    if subpackages:
        subpackages_json = ctx.actions.declare_file(pkg_dir + "/subpackages.json")
        ctx.actions.write(
            subpackages_json,
            content = json.encode_indent([{
                "package_manifest_file": subpackage[FuchsiaPackageInfo].package_manifest.path,
            } for subpackage in subpackages]),
        )

        subpackages_args = ["--subpackages-build-manifest-path", subpackages_json.path]
        subpackages_inputs = [subpackages_json] + [
            file
            for subpackage in subpackages
            for file in subpackage[FuchsiaPackageInfo].files
        ]

    # Validate binary paths in cmls
    component_manifest_files = [c.component_info.manifest for c in packaged_components]
    depfile = ctx.actions.declare_file(pkg_dir + "components_validation.depfile")
    ctx.actions.run(
        executable = ctx.executable._validate_component_manifests,
        arguments = [
            "--cmc",
            cmc_tool.path,
            "--component-manifest-paths",
            ",".join([c.path for c in component_manifest_files]),
            "--package-manifest",
            manifest.path,
            "--output",
            depfile.path,
        ],
        inputs = [cmc_tool, manifest] + component_manifest_files,
        outputs = [depfile],
        mnemonic = "CmcValidate",
        progress_message = "Validating binary paths in cml for %s" % ctx.label,
    )

    # Build the package
    build_args = []
    build_outputs = [
        output_package_manifest,
        meta_far,
    ]
    if ffx_package_is_ffx:
        ffx_isolate_build_dir = ctx.actions.declare_directory(pkg_dir + "_package_build.ffx")
        build_args += [
            "--isolate-dir",
            ffx_isolate_build_dir.path,
        ]
        build_outputs.append(ffx_isolate_build_dir)

    build_args += [
        "package",
        "build",
        manifest.path,
        "-o",
        output_package_manifest.dirname,
        "--published-name",
        ctx.attr.package_name,
    ]
    if api_level:
        build_args += ["--api-level", api_level]
    build_args += subpackages_args + repo_name_args

    # Build the package
    ctx.actions.run(
        executable = ffx_package,
        arguments = build_args,
        inputs = build_inputs + subpackages_inputs + [depfile],
        outputs = build_outputs,
        mnemonic = "FuchsiaPackageBuild",
        progress_message = "Building package for %s" % ctx.label,
        **LOCAL_ONLY_ACTION_KWARGS
    )

    artifact_inputs = [r.src for r in package_resources] + [
        output_package_manifest,
        meta_far,
    ] + subpackages_inputs

    # Create the far file.
    archive_args = []
    archive_outputs = [far_file]
    if ffx_package_is_ffx:
        ffx_isolate_archive_dir = ctx.actions.declare_directory(pkg_dir + "_package_archive.ffx")
        archive_args += [
            "--isolate-dir",
            ffx_isolate_archive_dir.path,
        ]
        archive_outputs.append(ffx_isolate_archive_dir)

    archive_args += [
        "package",
        "archive",
        "create",
        output_package_manifest.path,
        "-o",
        far_file.path,
    ]

    ctx.actions.run(
        executable = ffx_package,
        arguments = archive_args,
        inputs = artifact_inputs,
        outputs = archive_outputs,
        mnemonic = "FuchsiaPackageArchiveCreate",
        progress_message = "Archiving package for %{label}",
        **LOCAL_ONLY_ACTION_KWARGS
    )

    output_files = [
        far_file,
        output_package_manifest,
        manifest,
        meta_far,
    ] + build_inputs

    # Sanity check that we are not trying to put 2 different resources at the same mountpoint
    resource_dest_to_srcs = {}
    for resource in package_resources:
        resource_dest_to_srcs.setdefault(resource.dest, []).append(resource.src.path)
    for dest, srcs in resource_dest_to_srcs.items():
        if len(srcs) > 1:
            fail(
                "Multiple files are being installed into the package at {}:\n - {}".format(
                    dest,
                    "\n - ".join(srcs),
                ),
            )

    return [
        DefaultInfo(files = depset(output_files), executable = stub_executable(ctx)),
        FuchsiaPackageInfo(
            fuchsia_cpu = fuchsia_cpu_from_ctx(ctx),
            far_file = far_file,
            package_manifest = output_package_manifest,
            files = [output_package_manifest, meta_far] + build_inputs,
            package_name = ctx.attr.package_name,
            meta_far = meta_far,
            package_resources = package_resources,
            packaged_components = packaged_components,
            build_id_dirs = fuchsia_debug_symbol_info.build_id_dirs_mapping.values(),
        ),
        fuchsia_debug_symbol_info,
        OutputGroupInfo(
            build_id_dirs = depset(transitive = fuchsia_debug_symbol_info.build_id_dirs_mapping.values()),
        ),
    ]
