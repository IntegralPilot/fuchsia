# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Enumerates all components within a test package and runs each of them as test components."""

load("@fuchsia_rules_common//packages:providers.bzl", "FuchsiaPackageInfo")
load("//fuchsia/constraints:target_compatibility.bzl", "COMPATIBILITY")
load("//fuchsia/private:fuchsia_toolchains.bzl", "FUCHSIA_TOOLCHAIN_DEFINITION", "get_fuchsia_sdk_toolchain")
load(":fuchsia_task.bzl", "fuchsia_task_rule")

def _fuchsia_task_test_enumerated_components_impl(ctx, make_fuchsia_task):
    sdk = get_fuchsia_sdk_toolchain(ctx)
    repo = ctx.attr.repository
    package = getattr(ctx.attr.package[FuchsiaPackageInfo], "package_name", "{{PACKAGE_NAME}}")
    package_manifest = ctx.attr.package[FuchsiaPackageInfo].package_manifest
    package_archive = ctx.attr.package[FuchsiaPackageInfo].far_file
    url = "fuchsia-pkg://%s/%s#{{META_COMPONENT}}" % (repo, package)
    args = [
        "--ffx-test",
        sdk.ffx_test,
        "--ffx-package",
        sdk.ffx_package,
        "--url",
        url,
        "--package-manifest",
        package_manifest,
        "--package-archive",
        package_archive,
        "--match-component-name",
        ctx.attr.component_name_filter,
        "--retries",
        str(ctx.attr.retries),
    ]
    if ctx.attr.test_realm:
        args += [
            "--realm",
            ctx.attr.test_realm,
        ]
    if ctx.attr.disable_retries_on_failure:
        args.append("--disable-retries-on-failure")
    return make_fuchsia_task(
        ctx.attr._test_enumerated_components_tool,
        args,
        runfiles = [
            sdk.ffx_package_fho_meta,
            sdk.ffx_package_manifest,
            sdk.ffx_test_fho_meta,
            sdk.ffx_test_manifest,
        ],
    )

(
    _fuchsia_task_test_enumerated_components,
    _fuchsia_task_test_enumerated_components_for_test,
    fuchsia_task_test_enumerated_components,
) = fuchsia_task_rule(
    implementation = _fuchsia_task_test_enumerated_components_impl,
    toolchains = [FUCHSIA_TOOLCHAIN_DEFINITION],
    attrs = {
        "component_name_filter": attr.string(
            doc = "A regex filter allowlist applied to component names; used to filter components for testing.",
            default = ".+",
        ),
        "repository": attr.string(
            doc = "The repository that has the published package.",
            mandatory = True,
        ),
        "package": attr.label(
            doc = "The package containing the components to enumerate and test.",
            providers = [FuchsiaPackageInfo],
            mandatory = True,
        ),
        "test_realm": attr.string(
            doc = "Specify --realm to `ffx test run`.",
        ),
        "retries": attr.int(
            doc = "Specify an additional amount of test attempts per test failure.",
        ),
        "disable_retries_on_failure": attr.bool(
            doc = """Turns off test retrying for all tests once a test actually fails by reaching the `--retries` cap.

            Useful for preventing an excessive amount of test retries for real caught regressions.""",
        ),
        "_test_enumerated_components_tool": attr.label(
            doc = "The tool used to enumerate and test all components",
            default = "//fuchsia/tools:test_enumerated_components",
            executable = True,
            cfg = "exec",
        ),
    } | COMPATIBILITY.HOST_ATTRS,
)
