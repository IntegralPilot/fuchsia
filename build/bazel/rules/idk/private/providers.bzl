# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Defines providers related to the IDK."""

# LINT.IfChange(idk_atom_info)
# TOOD(https://fxbug.dev/417304469): `api_area`,  and some other
# fields of this provider do not belong in prebuild info. `deps` may
# also be unnecessary, but could be useful for category enforcement.
FuchsiaIdkAtomInfo = provider(
    doc = "Defines an IDK atom",
    fields = {
        "label": "[label] The atom's label",
        "idk_name": "[string] Name of this atom within the IDK",
        "id": "[string] Identifier of this atom within the IDK",
        "meta_dest": "[string] Location of the atom's metadata file in the final IDK",
        "type": "[string] The type of atom",
        "category": "[string] The IDK category for the atom",
        "is_stable": "[bool] Whether the atom is stable",
        "api_area": "[string] The API area responsible for maintaining this atom",
        "api_file_path": "Path to the file representing the API canonically exposed by this atom.",
        "api_contents_map": "List of scopes for the files making up the atom's API.",
        "atom_files_map": "[dict[str,File]] a { dest -> source } map of files for this atom",
        "deps": "[list[label]] Other atoms the atom directly depends on",
        "atoms_depset": "[depset[FuchsiaIdkAtomInfo]] The full set of other atoms the atom depends on",
        "atom_build_deps": "[list[label]] List of dependencies related to building the atom that should not be reflected in IDKs",
        "additional_prebuild_info": "[dict[str,list[Any]]] A dictionary of type-specific prebuild info for the atom. All values are lists, even if there is only one value",
    },
)
# LINT.ThenChange(//build/bazel/rules/idk/private/idk_atom.bzl:idk_atom_info,//build/bazel/bazel_idk/tests/rules.bzl:idk_atom_info)

FuchsiaIdkMoleculeInfo = provider(
    doc = "Defines an IDK molecule, or group of atoms",
    fields = {
        "label": "The molecule's label",
        "deps": "Atoms and other molecules the molecule depends on.",
        "atoms_depset": "depset[FuchsiaIdkAtomInfo] The full set atoms that make up the molecule",
    },
)
