# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Common implementation for `fuchsia_*_component()` macros. Shared by platform and the Bazel SDK."""

load(
    "@fuchsia_rules_common//:utils.bzl",
    "make_resource_struct",
)
load(
    "@fuchsia_rules_common//debug_symbols:debug_symbols.bzl",
    "merge_debug_symbol_infos",
)
load(
    "@fuchsia_rules_common//packages:providers.bzl",
    "FuchsiaPackageResourcesInfo",
)
load(
    ":providers.bzl",
    "FuchsiaComponentInfo",
    "FuchsiaComponentManifestInfo",
)

def _make_fuchsia_component_providers(*, component_name, manifest, resources, is_driver, is_test, moniker, run_tag):
    return [
        FuchsiaComponentInfo(
            name = component_name,
            manifest = manifest,
            resources = resources,
            is_driver = is_driver,
            is_test = is_test,
            moniker = moniker,
            run_tag = run_tag,
        ),
        FuchsiaPackageResourcesInfo(resources = [
            make_resource_struct(
                src = manifest,
                dest = "meta/{}".format(manifest.basename),
            ),
        ]),
    ]

def _fuchsia_component_common_impl(ctx):
    # TODO(http://b/525461025): Also check for session components per the doc string.
    if ctx.attr.moniker and (ctx.attr.is_driver or ctx.attr.is_test):
        fail("`moniker` should not be set for driver or test components.")

    component_name = ctx.attr.component_name or ctx.attr.compiled_manifest[FuchsiaComponentManifestInfo].component_name
    manifest = ctx.attr.compiled_manifest[FuchsiaComponentManifestInfo].compiled_manifest

    resources = []
    for dep in ctx.attr.deps:
        if FuchsiaPackageResourcesInfo in dep:
            resources += dep[FuchsiaPackageResourcesInfo].resources
        else:
            for mapping in dep[DefaultInfo].default_runfiles.root_symlinks.to_list():
                resources.append(make_resource_struct(src = mapping.target_file, dest = mapping.path))

            for f in dep.files.to_list():
                resources.append(make_resource_struct(src = f, dest = f.short_path))

    return _make_fuchsia_component_providers(
        component_name = component_name,
        manifest = manifest,
        resources = resources,
        is_driver = ctx.attr.is_driver,
        is_test = ctx.attr.is_test,
        moniker = ctx.attr.moniker.format(COMPONENT_NAME = component_name),
        run_tag = ctx.label.name,
    ) + [
        merge_debug_symbol_infos(ctx.attr.deps),
    ]

fuchsia_component_common = rule(
    doc = """Creates a Fuchsia component which can be added to a package

This rule will take a component manifest and compile it into a form that
is suitable to be included in a package. The component can include any
number of dependencies which will be included in the final package.
""",
    implementation = _fuchsia_component_common_impl,
    # Prefer mandatory attributes to ensure wrapping macros intentionally set them.
    attrs = {
        "deps": attr.label_list(
            doc = """A list of targets that this component depends on.

            The necessary files for each target will be included in the final package.
            """,
            mandatory = False,
        ),
        "moniker": attr.string(
            doc = """The moniker to run the component under.

            Instances of `{COMPONENT_NAME}` are replaced with the component name.

            Use only for non-test, non-driver, and non-session components.
            """,
            # Cannot be mandatory because it is a string.
            mandatory = False,
        ),
        "compiled_manifest": attr.label(
            doc = "The `fuchsia_component_manifest` target.",
            providers = [FuchsiaComponentManifestInfo],
            mandatory = True,
        ),
        "component_name": attr.string(
            doc = """The name of the component.

            This value will override the component name value in the `compiled_manifest`.
            Defaults to the basename of the `compiled_manifest` file.
            """,
            mandatory = False,
        ),
        "is_driver": attr.bool(
            doc = """True if this is a driver component.

            Controls how the SDK runs the component.
            """,
            mandatory = True,
        ),
        "is_test": attr.bool(
            doc = """True if this is a test component.

            Controls how the SDK runs the component.
            This is independent of the `testonly` attribute.
            """,
            mandatory = True,
        ),
    },
)
