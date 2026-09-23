# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Repository rules used to populate Developer Overrides for assembly"""

def _make_overrides_target(label):
    target_lines = [
        "fuchsia_prebuilt_assembly_developer_overrides(",
        "    name = \"%s\"," % label.name,
        "    overrides_path = \"@gn_targets//%s:%s\"," % (label.package, label.name),
        ")",
    ]
    return target_lines

def _generate_developer_overrides_repository_impl(repo_ctx):
    overrides_map_file = repo_ctx.path(Label("@//:" + repo_ctx.attr.overrides_map_from_gn))
    overrides_map_data = repo_ctx.read(overrides_map_file)
    overrides_map_gn = json.decode(overrides_map_data)

    buildfile_lines = [
        "# DO NOT EDIT! Automatically generated.",
        "",
        "load(",
        "    \"@rules_fuchsia//fuchsia:assembly.bzl\",",
        "    \"fuchsia_assembly_developer_overrides_list\",",
        "    \"fuchsia_prebuilt_assembly_developer_overrides\",",
        ")",
        "",
        "",
    ]

    overrides_list = []
    overrides_targets_created = []
    for overrides_entry in overrides_map_gn:
        override_label = Label(overrides_entry["overrides"])

        # If the target hasn't been defined, yet, then do so.
        if not override_label in overrides_targets_created:
            target_lines = _make_overrides_target(override_label)
            buildfile_lines.extend(target_lines)
            buildfile_lines.append("")
            overrides_targets_created.append(override_label)

        # Add the mapping to the list of mappings.
        overrides_list.append({
            "assembly": overrides_entry["assembly"],
            "override_label": override_label,
        })

    buildfile_lines.append("")
    buildfile_lines.extend([
        "fuchsia_assembly_developer_overrides_list(",
        "    name = \"in-tree_developer_overrides_list\",",
        "    maps = [",
    ])
    for mapping in overrides_list:
        buildfile_lines.extend([
            "        {",
            "            \"assembly\": \"%s\"," % mapping["assembly"],
            "            \"overrides\": \":%s\"," % mapping["override_label"].name,
            "        },",
        ])
    buildfile_lines.extend([
        "    ]",
        ")",
        "",
    ])

    repo_ctx.file("BUILD.bazel", "\n".join(buildfile_lines))

assembly_developer_overrides_repository = repository_rule(
    implementation = _generate_developer_overrides_repository_impl,
    attrs = {
        "overrides_map_from_gn": attr.string(
            doc = "Path to the GN-written map of assembly developer overrides",
            mandatory = True,
        ),
    },
)
