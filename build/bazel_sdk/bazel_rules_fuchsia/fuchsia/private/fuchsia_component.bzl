# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Macros for defining Fuchsia components in the Bazel SDK."""

load(
    "@fuchsia_rules_common//components:fuchsia_component_common.bzl",
    "fuchsia_component_common",
)
load(":fuchsia_component_manifest.bzl", "ensure_compiled_component_manifest")

def _manifest_target(name, manifest_in, tags, testonly):
    target_name = name + "_ensure_compiled_manifest"
    ensure_compiled_component_manifest(
        name = target_name,
        dep = manifest_in,
        testonly = testonly,
        tags = tags + ["manual"],
    )
    return target_name

def fuchsia_component(
        *,
        name,
        manifest,
        moniker = "/core/ffx-laboratory:{COMPONENT_NAME}",
        component_name = None,
        deps = [],
        tags = ["manual"],
        **kwargs):
    """Creates a Fuchsia component that can be added to a package.

    Args:
        name: The target name.
        manifest: The component manifest file.
            This attribute can be a `fuchsia_component_manifest` target or a `.cml`
            file. If a `.cml` file is provided it will be compiled into a `.cm` file.
            If `component_name` is provided, the generated `.cm` file will
            inherit that name. Otherwise, it will keep the same basename.
            TODO(http://b/525461025): Implement the `component_name` behavior for the `.cm` file.

            If you need to have more control over the compilation of the `.cm` file
            we suggest you create a `fuchsia_component_manifest` target and pass
            it to this rule.
        moniker: The moniker to run the component under.
            Defaults to "/core/ffx-laboratory:{COMPONENT_NAME}".
        component_name: The name of the component.
            Defaults to the component manifest file's basename.
        deps: A list of targets that this component depends on.
        tags: Standard meaning. Defaults to `["manual"]`.
        **kwargs: Extra attributes to forward to the build rule.
    """

    # Prevent callers from passing through these attributes, which are set appropriately below.
    for attr in ["is_driver", "is_test"]:
        if attr in kwargs:
            fail("Attribute `%s` is not supported. Use the appropriate macro instead." % attr)

    manifest_target = _manifest_target(name, manifest, tags, testonly = False)

    fuchsia_component_common(
        name = name,
        moniker = moniker,
        compiled_manifest = manifest_target,
        component_name = component_name,
        deps = deps,
        tags = tags,
        is_driver = False,
        is_test = False,
        **kwargs
    )

def fuchsia_test_component(
        *,
        name,
        manifest,
        component_name = None,
        deps = [],
        tags = ["manual"],
        **kwargs):
    """Creates a Fuchsia component that can be added to a test package.

    See `fuchsia_component` for more information.

    Args:
        name: The target name.
        manifest: The component manifest file.
        component_name: The name of the component.
        deps: A list of targets that this component depends on.
        tags: Standard meaning. Defaults to `["manual"]`.
        **kwargs: Extra attributes to forward to the build rule.
    """

    # Prevent passing through these attributes, which are set appropriately below.
    for attr in ["moniker", "is_driver", "is_test"]:
        if attr in kwargs:
            fail("Attribute `%s` is not supported." % attr)

    manifest_target = _manifest_target(name, manifest, tags, testonly = True)

    fuchsia_component_common(
        name = name,
        moniker = None,
        compiled_manifest = manifest_target,
        component_name = component_name,
        deps = deps,
        tags = tags,
        is_driver = False,
        is_test = True,
        testonly = True,
        **kwargs
    )

def fuchsia_driver_component(
        *,
        name,
        manifest,
        driver_lib,
        bind_bytecode,
        component_name = None,
        deps = [],
        tags = ["manual"],
        **kwargs):
    """Creates a Fuchsia component that can be registered as a driver.

    See `fuchsia_component` for more information.

    Args:
        name: The target name.
        manifest: The component manifest file.
        driver_lib: The shared library that will be registered with the driver manager.
           This file will end up in /driver/<lib_name> and should match what is listed
           in the manifest. See https://fuchsia.dev/fuchsia-src/concepts/components/v2/driver_runner
           for more details.
        bind_bytecode: The driver bind bytecode needed for binding the driver.
        component_name: The name of the component.
        deps: A list of targets that this component depends on.
        tags: Standard meaning. Defaults to `["manual"]`.
        **kwargs: Extra attributes to forward to the build rule.
    """

    # Prevent passing through these attributes, which are set appropriately below.
    for attr in ["moniker", "is_driver", "is_test"]:
        if attr in kwargs:
            fail("Attribute `%s` is not supported." % attr)

    manifest_target = _manifest_target(name, manifest, tags, testonly = False)

    fuchsia_component_common(
        name = name,
        moniker = None,
        compiled_manifest = manifest_target,
        component_name = component_name,
        deps = deps + [
            bind_bytecode,
            driver_lib,
        ],
        tags = tags,
        is_driver = True,
        is_test = False,
        **kwargs
    )
