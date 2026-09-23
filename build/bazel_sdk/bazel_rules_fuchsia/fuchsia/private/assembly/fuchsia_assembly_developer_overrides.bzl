# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rules related to assembly developer overrides."""

load(
    ":providers.bzl",
    "FuchsiaAssemblyDeveloperOverridesInfo",
    "FuchsiaAssemblyDeveloperOverridesListInfo",
)
load(
    ":utils.bzl",
    "select_single_file",
)

def _fuchsia_prebuilt_assembly_developer_overrides_impl(ctx):
    manifest = select_single_file(ctx.files.overrides_path, "product_assembly_overrides.json")
    if not manifest:
        fail("Unable to locate 'product_assembly_overrides.json' in Assembly Developer Overrides input.")

    return [
        FuchsiaAssemblyDeveloperOverridesInfo(
            manifest = manifest,
            inputs = ctx.files.overrides_path,
        ),
    ]

_fuchsia_prebuilt_assembly_developer_overrides = rule(
    doc = "Records information about a set of prebuilt developer overrides",
    implementation = _fuchsia_prebuilt_assembly_developer_overrides_impl,
    attrs = {
        "overrides_path": attr.label(
            doc = "A label to fuchsia_assembly_developer_overrides",
            mandatory = True,
        ),
    },
)

def fuchsia_prebuilt_assembly_developer_overrides(name, overrides_path):
    """Record information about a set of prebuilt (from GN?) assembly developer overrides.

    Args:
        name: target name.
        overrides_path: A file glob to the folder that contains the manifest
            and all resources that are referred to by the manifest.
    """
    _fuchsia_prebuilt_assembly_developer_overrides(
        name = name,
        overrides_path = overrides_path,
    )

def _fuchsia_assembly_developer_overrides_list_impl(ctx):
    maps = {}
    for (label, assembly_targets_json) in ctx.attr.developer_overrides.items():
        assembly_targets = json.decode(assembly_targets_json)
        for assembly in assembly_targets:
            if assembly in maps:
                fail("Found duplicate developer overrides for assembly: %s" % assembly)
            maps[assembly] = label

    return FuchsiaAssemblyDeveloperOverridesListInfo(
        maps = maps,
    )

_fuchsia_assembly_developer_overrides_list = rule(
    doc = "Record information about a list of mappings for developer overrides. Uses two parallel lists of equal sizes.",
    implementation = _fuchsia_assembly_developer_overrides_list_impl,
    attrs = {
        "developer_overrides": attr.label_keyed_string_dict(
            doc = "List of labels to fuchsia_assembly_developer_overrides() targets.",
            default = {},
            providers = [FuchsiaAssemblyDeveloperOverridesInfo],
        ),
    },
)

def fuchsia_assembly_developer_overrides_list(name, maps = []):
    """Record information about a list of assembly developer overrides and the assembly product labels they apply to.

    Example usage:

        ```
        fuchsia_assembly_developer_overrides_list(
            name = "my_project_overrides",
            maps = [
                {
                    "assembly": "//products/foo:*",
                    "overrides": ":foo_assembly_overrides",
                },
            ]
        )

        fuchsia_assembly_developer_overrides(
            name = "foo_assembly_overrides",
            developer_overrides_json = {
                "developer_only_options": {
                    "all_packages_in_base": True,
                },
                "base_packages": [
                   "LABEL(//packages/extras:bar)",
                ],
            },
        )
        ```

    Args:
       name: target name.
       maps: A list of dictionaries with this schema:

           assembly: A label pattern string that will be used
              to filter which fuchsia_product() targets the "overrides"
              value applies to.

           overrides: A label to a fuchsia_assembly_developer_overrides() target.

    """
    if type(maps) != "list":
        fail("expecting a list")

    developer_overrides = {}
    for mapping in maps:
        overrides = mapping["overrides"]
        if overrides in developer_overrides:
            assemblies = developer_overrides[overrides]
        else:
            assemblies = []
        assemblies.append(mapping["assembly"])
        developer_overrides[overrides] = assemblies

    _fuchsia_assembly_developer_overrides_list(
        name = name,
        # patterns = patterns,
        developer_overrides = {label: json.encode(assemblies) for label, assemblies in developer_overrides.items()},
    )
