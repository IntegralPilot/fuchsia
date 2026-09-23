# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Aspect and rules to collect FIDL JSON data."""

load("//build/bazel/rules/fidl:providers.bzl", "FidlLibraryInfo")
load("//build/bazel/rules/idk:providers.bzl", "FuchsiaIdkAtomInfo", "FuchsiaIdkMoleculeInfo")

visibility(["//build/fidl/..."])

FidlMetadataAspectInfo = provider(
    doc = "Collects struct metadata for FIDL libraries.",
    fields = {
        # LINT.IfChange(idk_fidl_json_data_fields)
        "metadata": """A depset of structs containing IDK FIDL JSON data collected.
            Each struct has the following fields:
            - name: Name of the FIDL library (string).
            - ir: Path to the FIDL IR JSON file relative to the source root (string).
            - category: The FIDL library's category if any (string).
            - sdk_area: The FIDL library's API area (string).
        """,
        # LINT.ThenChange(:sdk_fidl_json_data_contents, :sdk_fidl_json_data_json)
    },
)

def _fidl_metadata_aspect_impl(target, ctx):
    metadata = []

    if FuchsiaIdkAtomInfo in target:
        atom_info = target[FuchsiaIdkAtomInfo]
        if atom_info.type == "fidl_library":
            if not (hasattr(ctx.rule.attr, "atom_build_deps") and len(ctx.rule.attr.atom_build_deps) > 1):
                fail("FIDL library atom must have more than one atom_build_dep.")
            ir_file = None
            for dep in ctx.rule.attr.atom_build_deps:
                if FidlLibraryInfo in dep:
                    if len(dep[DefaultInfo].files.to_list()) != 1:
                        fail("`FidlLibraryInfo` target `%s` must have exactly one file." % dep.label)
                    ir_file = dep[DefaultInfo].files.to_list()[0]
                    if not ir_file.basename.endswith(".fidl.json"):
                        fail("Unexpected file `%s` found in `FidlLibraryInfo` target `%s.`" % (
                            ir_file.path,
                            dep.label,
                        ))
                    break

            if not ir_file:
                fail("Could not find IR JSON file in FIDL library atom.")

            # LINT.IfChange(idk_fidl_json_data_contents)
            metadata.append(struct(
                # The IDK name is the library name.
                name = atom_info.idk_name,
                ir = ir_file.path,
                category = atom_info.category,
                sdk_area = getattr(atom_info, "api_area", "Unknown"),
            ))
            # LINT.ThenChange(:sdk_fidl_json_data_fields, :sdk_fidl_json_data_json, //build/fidl/fidl_library.gni:sdk_fidl_json_data_contents)

    transitive = []
    for dep in ctx.rule.attr.deps:
        if FidlMetadataAspectInfo in dep:
            transitive.append(dep[FidlMetadataAspectInfo].metadata)

    return [FidlMetadataAspectInfo(
        metadata = depset(direct = metadata, transitive = transitive),
    )]

# Collects metadata for FIDL library IDK atoms.
# This aspect only traverses IDK molecules and IDK atoms. Both reference
# other such targets via `deps`.
fidl_metadata_aspect = aspect(
    implementation = _fidl_metadata_aspect_impl,
    attr_aspects = ["deps"],
)

def _collect_fidl_metadata_impl(ctx):
    metadata_dict = {}

    for dep in ctx.attr.deps:
        if FidlMetadataAspectInfo in dep:
            for data in dep[FidlMetadataAspectInfo].metadata.to_list():
                metadata_dict[data.name] = data

    json_data = [
        # LINT.IfChange(idk_fidl_json_data_json)
        {
            "name": entry.name,
            "ir": entry.ir,
            "category": entry.category,
            "sdk_area": entry.sdk_area,
        }
        # LINT.ThenChange(:sdk_fidl_json_data_fields, :sdk_fidl_json_data_contents, //build/fidl/fidl_library.gni:sdk_fidl_json_data_contents)
        for entry in metadata_dict.values()
        if entry.category in ctx.attr.categories
    ]
    json_data = sorted(json_data, key = lambda x: x["name"])

    out_file = ctx.actions.declare_file(ctx.label.name + ".json")
    ctx.actions.write(out_file, json.encode_indent(json_data, indent = "  "))
    return [DefaultInfo(files = depset([out_file]))]

collect_fidl_metadata = rule(
    doc = "Collects metadata for FIDL library IDK atoms in a dependency graph into `<name>.json`. " +
          "Only follows public `deps`. Thus, it does not collect privately " +
          "used FIDL libraries, such as those used in the implementation of " +
          "a prebuilt library IDK atom.",
    implementation = _collect_fidl_metadata_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "List of dependency graph root IDK atoms from which to collect metadata.",
            providers = [[FuchsiaIdkAtomInfo], [FuchsiaIdkMoleculeInfo]],
            aspects = [fidl_metadata_aspect],
            mandatory = True,
        ),
        "categories": attr.string_list(
            doc = "Categories for which to collect metadata. Libraries in other categories will be ignored.",
            mandatory = True,
            allow_empty = False,
        ),
    },
)

# To locate information about a FIDL library in `FidlLibraryInfo`, we have to
# find the FIDL IR JSON target. Thus, we must ensure that the attribute names
# for all possible paths to that target have the aspect applied and are checked
# in the aspect.
# TODO(https://fxbug.dev/496603528): Find a more robust way to ensure that
# `FidlLibraryInfo` is found. Otherwise, this is fragile and must cover all
# bindings types.
_dependency_attrs_to_check_for_fidl_library_info = [
    # Normal dependency attributes.
    "deps",
    "implementation_deps",
    "data",

    # The FIDL IR target is added to the FIDL atom's `atom_build_deps`.
    "atom_build_deps",

    # Attributes used in the generation of [hl]cpp bindings.
    "fidl_ir_json",
    "generated_fidl_cc_bindings",
    "hdrs",
    "srcs",
]

FidlIrJsonAspectInfo = provider(
    doc = "Collects the paths of IR JSON files.",
    fields = {
        "ir_json_files": """A depset of string paths for IR files collected.
            Paths are relative to the source root.
        """,
    },
)

def _fidl_ir_json_aspect_impl(target, ctx):
    ir_json_files = []

    if FidlLibraryInfo in target:
        for f in target[DefaultInfo].files.to_list():
            if f.basename.endswith(".fidl.json"):
                ir_json_files.append(f)
                break

    transitive = []

    for attr_name in _dependency_attrs_to_check_for_fidl_library_info:
        if hasattr(ctx.rule.attr, attr_name):
            attr_value = getattr(ctx.rule.attr, attr_name)
            dep_targets = attr_value if type(attr_value) == "list" else [attr_value]
            for dep in dep_targets:
                if FidlIrJsonAspectInfo in dep:
                    transitive.append(dep[FidlIrJsonAspectInfo].ir_json_files)

    return [FidlIrJsonAspectInfo(
        ir_json_files = depset(direct = ir_json_files, transitive = transitive),
    )]

# Collects the paths of IR JSON files.
fidl_ir_json_aspect = aspect(
    implementation = _fidl_ir_json_aspect_impl,
    attr_aspects = _dependency_attrs_to_check_for_fidl_library_info,
)

def _collect_fidl_ir_json_files_impl(ctx):
    ir_json_files_set = set()

    for dep in ctx.attr.deps:
        if FidlIrJsonAspectInfo in dep:
            ir_json_files_set.update([
                file.path
                for file in dep[FidlIrJsonAspectInfo].ir_json_files.to_list()
            ])

    paths = sorted(ir_json_files_set)
    out_file = ctx.actions.declare_file(ctx.label.name + ".txt")
    ctx.actions.write(out_file, "\n".join(paths) + "\n")
    return [DefaultInfo(files = depset([out_file]))]

collect_fidl_ir_json_files = rule(
    doc = """Collects the paths of all FIDL IR JSON files in a dependency graph into `<name>.txt`.
    The files are listed unstructured, one per line. Paths are relative to the source root.
    """,
    implementation = _collect_fidl_ir_json_files_impl,
    attrs = {
        "deps": attr.label_list(
            doc = "List of dependency graph root targets from which to collect IR JSON files.",
            aspects = [fidl_ir_json_aspect],
            mandatory = True,
            allow_empty = False,
        ),
    },
)
