#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Functionality for building the IDK from its component parts."""

# See https://stackoverflow.com/questions/33533148/how-do-i-type-hint-a-method-with-the-type-of-the-enclosing-class
from __future__ import annotations

import dataclasses
import filecmp
import itertools
import json
import pathlib
from collections.abc import Callable, Mapping, Sequence
from typing import (
    Any,
    Literal,
    Optional,
    TypedDict,
    TypeVar,
    get_args,
    get_type_hints,
)

from .validate_idk import *

# version_history.json doesn't follow the same schema as other IDK metadata
# files, so we treat it specially.
VERSION_HISTORY_PATH = pathlib.Path("version_history.json")


# These atom types include a "stable" field because they support unstable atoms.
TYPES_SUPPORTING_UNSTABLE_ATOMS = [
    # LINT.IfChange(unstable_atom_types)
    "cc_source_library",
    "fidl_library"
    # LINT.ThenChange(//build/bazel/bazel_idk/private/idk_atom.bzl:unstable_atom_types, //build/sdk/sdk_atom.gni:unstable_atom_types, //build/sdk/generate_prebuild_idk/idk_generator.py)
]


class IdkManifestJson(TypedDict):
    """A type description of a subset of the fields in the IDK manifest.

    We don't explicitly check that the manifests in question actually match this
    schema - we just assume it.
    """

    parts: list[IdkManifestAtom]


class IdkManifestAtom(TypedDict):
    meta: str
    stable: bool
    type: str


class AtomFile(TypedDict):
    source: str
    destination: str


# The next few types describe a subset of the fields of various atom manifests,
# as they will be included in the IDK.


class CCPrebuiltLibraryMeta(TypedDict):
    name: str
    type: Literal["cc_prebuilt_library"]
    binaries: dict[str, Any]
    variants: list[Any]
    stable: bool


class SysrootMeta(TypedDict):
    name: str
    type: Literal["sysroot"]
    versions: dict[str, Any]
    variants: list[Any]
    stable: bool


class PackageMeta(TypedDict):
    name: str
    type: Literal["package"]
    variants: list[Any]
    stable: bool


class LoadableModuleMeta(TypedDict):
    name: str
    type: Literal["loadable_module"]
    binaries: dict[str, Any]
    stable: bool


class UnmergeableMeta(TypedDict):
    name: str
    type: Literal[
        "bind_library",
        "cc_source_library",
        "component_manifest",
        "config",
        "dart_library",
        "documentation",
        "experimental_python_e2e_test",
        "fidl_library",
        "license",
        "version_history",
    ]
    stable: bool


AtomMeta = (
    CCPrebuiltLibraryMeta
    | LoadableModuleMeta
    | PackageMeta
    | SysrootMeta
    | UnmergeableMeta
)


@dataclasses.dataclass
class PartialAtom:
    """Metadata and files associated with a single Atom from a single subbuild.

    Attributes:
        meta (AtomMeta): JSON object from the atom's `meta.json` file.
        meta_src (pathlib.Path): The path from which `meta` was read.
        dest_to_src (dict[pathlib.Path, pathlib.Path]): All non-metadata files
            associated with this atom belong in this dictionary. The key is the
            file path relative to the final IDK directory. The value is either
            absolute or relative to the current working directory.
    """

    meta: AtomMeta
    meta_src: pathlib.Path
    dest_to_src: dict[pathlib.Path, pathlib.Path]


@dataclasses.dataclass
class PartialIDK:
    """A model of the parts of an IDK from a single subbuild.

    Attributes:
        manifest_src (pathlib.Path): Source path for the overall IDK manifest.
            Either absolute or relative to the current working directory.
        atoms (dict[pathlib.Path, PartialAtom]): Atoms to include in the IDK,
            indexed by the path to their metadata file, relative to the final
            IDK directory (e.g., `bind/fuchsia.ethernet/meta.json`).
    """

    manifest_src: pathlib.Path
    atoms: dict[pathlib.Path, PartialAtom]

    @staticmethod
    def load(collection_path: pathlib.Path) -> PartialIDK:
        """Load relevant information about a piece of the IDK from a collection
        in a subbuild directory.

        Args:
            collection_path: Path to the directory containing the collection.
        Returns:
            A PartialIDK representing the collection.
        """

        result = PartialIDK(
            manifest_src=(collection_path / "meta/manifest.json"), atoms={}
        )
        with (result.manifest_src).open() as f:
            idk_manifest: IdkManifestJson = json.load(f)

        for atom in idk_manifest["parts"]:
            meta_dest = pathlib.Path(atom["meta"])
            meta_src = collection_path / meta_dest
            dest_to_src = {}

            atom_type = atom["type"]

            with meta_src.open() as f:
                atom_meta = json.load(f)

            if meta_dest == VERSION_HISTORY_PATH:
                # version_history.json doesn't have a 'type' field, so set
                # it. See https://fxbug.dev/409622622.
                assert "type" not in atom_meta.keys()
                atom_meta["type"] = "version_history"

            files_list = result._get_file_list(atom_type, atom_meta)

            for file in files_list:
                src_path = collection_path / file
                dest_path = pathlib.Path(file)

                assert dest_path not in dest_to_src, (
                    "File specified multiple times in atom: %s" % dest_path
                )
                dest_to_src[dest_path] = src_path

            assert meta_dest not in result.atoms, (
                "Atom metadata file specified multiple times: %s" % meta_dest
            )

            result.atoms[meta_dest] = PartialAtom(
                meta=atom_meta,
                meta_src=meta_src,
                dest_to_src=dest_to_src,
            )

        return result

    @staticmethod
    def _get_file_list(atom_type: str, atom_meta: dict[str, Any]) -> list[str]:
        """Obtains a list of files from `atom_meta`.

        Atom types specify the files corresponding to the atom in various ways.
        This function handles all of them.

        Args:
            atom_type: The atom type as used in the IDK manifest.
            atom_meta: The atom metadata from its meta.json file.
        Returns:
            A list of the file path for the atom. Paths are relative to the IDK
            root directory.
        """
        files_list = []

        # First handle common fields that can coexist with others.
        if "headers" in atom_meta:
            assert atom_type in [
                "cc_prebuilt_library",
                "cc_source_library",
            ], f"Unexpected entry for {atom_type}"

            # Contains a list of files.
            files_list += atom_meta["headers"]

        # The following fields are mutually exclusive.
        if atom_type == "version_history":
            # Version history is special. It contains "data", so we must
            # avoid matching that case.
            pass
        elif "sources" in atom_meta:
            assert atom_type in [
                "dart_library",
                "cc_source_library",
                "fidl_library",
                "bind_library",
            ], f"Unexpected entry for {atom_type}"

            # Contains a list of files.
            files_list += atom_meta["sources"]
        elif "files" in atom_meta:
            assert atom_type in [
                "host_tool",
                "companion_host_tool",
                "experimental_python_e2e_test",
                "ffx_tool",
            ], f"Unexpected entry for {atom_type}"

            # Contains a list of files.
            files_list += atom_meta["files"]

            # Handle ffx_tool, which has another entry.
            if "target_files" in atom_meta:
                assert (
                    atom_type == "ffx_tool"
                ), f"Unexpected entry for {atom_type}"
                # Contains a dictionary of variants.
                assert (
                    len(atom_meta["target_files"]) == 1
                ), "Expected only one variant per sub-build."
                variant = list(atom_meta["target_files"].values())[0]

                # The variant contains a dictionary of file entries.
                files_list += []
                for label, file in variant.items():
                    files_list.append(file)
        elif "data" in atom_meta:
            assert atom_type == "data", f"Unexpected entry for {atom_type}"

            # Contains a list of files.
            files_list += atom_meta["data"]
        elif "docs" in atom_meta:
            assert (
                atom_type == "documentation"
            ), f"Unexpected entry for {atom_type}"

            # Contains a list of files.
            files_list += atom_meta["docs"]
        elif "variants" in atom_meta:
            assert (
                len(atom_meta["variants"]) == 1
            ), "Expected only one variant per sub-build."

            # Contains an array of items of various possible types.
            variant = atom_meta["variants"][0]

            if atom_type == "package":
                # The variant contains a list of files.
                files_list += variant["files"]
            elif atom_type == "sysroot":
                # The variant contains a dictionary of file lists
                files_list += []
                for label, file_list in variant["values"].items():
                    if label not in [
                        "include_dir",
                        "root",
                        "sysroot_dir",
                    ]:
                        files_list += file_list

                # The IFS files are listed separately outside the variant and
                # not versioned.
                # TODO(https://fxbug.dev/339884866): Version libzircon and get
                # its IFS file from the variant.
                # TODO(https://fxbug.dev/388856587): Version libc and get
                # its IFS file from the variant.
                files_list += atom_meta["ifs_files"]
            elif atom_type == "cc_prebuilt_library":
                files_list += []
                for label, file in variant["values"].items():
                    if label not in ["dist_lib_dest"]:
                        files_list.append(file)
            else:
                assert False, f"Unexpected entry for {atom_type}"
        elif "binaries" in atom_meta:
            # TODO(https://fxbug.dev/310006516): Remove this case when no longer
            # distributing API-level-agnostic prebuilts (arch/) in the IDK.

            assert (
                len(atom_meta["binaries"]) == 1
            ), "Expected only one variant per sub-build."

            # Contains a dictionary of variants.
            variant = list(atom_meta["binaries"].values())[0]

            if atom_type in ["cc_prebuilt_library"]:
                files_list += []
                for label, file in variant.items():
                    if label != "dist_path":
                        files_list.append(file)

                # There may be another entry, which is a file.
                format = atom_meta["format"]
                if "ifs" in atom_meta:
                    assert (
                        format == "shared"
                    ), f"Unexpected entry for {atom_type} format {format}"
                    files_list.append(atom_meta["ifs"])
                else:
                    assert (
                        format == "static"
                    ), f"Unexpected format {format} for {atom_type}"
            elif atom_type == "loadable_module":
                # The variant is just a list of files.
                files_list += variant

                # There is another entry, which is a list of files.
                files_list += atom_meta["resources"]
            else:
                assert False, f"Unexpected entry for {atom_type}"
        elif "versions" in atom_meta:
            # TODO(https://fxbug.dev/310006516): Remove this case when no longer
            # distributing API-level-agnostic prebuilts (arch/) in the IDK.

            assert atom_type == "sysroot", f"Unexpected entry for {atom_type}"

            # Contains a dictionary of variants.
            assert (
                len(atom_meta["versions"]) == 1
            ), "Expected only one variant per sub-build."
            variant = list(atom_meta["versions"].values())[0]

            # The variant contains a dictionary of file lists
            for label, file_list in variant.items():
                if label not in [
                    "dist_dir",
                    "include_dir",
                    "root",
                ]:
                    files_list += file_list

            # The IFS files are listed separately outside the variant.
            files_list += atom_meta["ifs_files"]
        else:
            assert False, f"Unhandleld atom type {atom_type}"

        return files_list

    def input_files(self) -> set[pathlib.Path]:
        """Return the set of input files in this PartialIDK for generating a
        depfile."""
        result = set()
        result.add(self.manifest_src)
        for atom in self.atoms.values():
            result.add(atom.meta_src)
            result |= set(atom.dest_to_src.values())
        return result


class AtomMergeError(Exception):
    def __init__(self, atom_path: pathlib.Path):
        super(AtomMergeError, self).__init__(
            "While merging atom: %s" % atom_path
        )


@dataclasses.dataclass
class MergedIDK:
    """A model of a (potentially incomplete) IDK.

    Attributes:
        atoms (dict[pathlib.Path, AtomMeta]): Atoms to include in the IDK,
            indexed by the path to their metadata file, relative to the final
            IDK directory (e.g., `bind/fuchsia.ethernet/meta.json`). The values
            are the parsed JSON objects that will be written to that path.
        dest_to_src (dict[pathlib.Path, pathlib.Path]): All non-metadata files
            in the IDK belong in this dictionary. The key is the file path
            relative to the final IDK directory. The value is either absolute or
            relative to the current working directory.
    """

    atoms: dict[pathlib.Path, AtomMeta] = dataclasses.field(
        default_factory=dict
    )
    dest_to_src: dict[pathlib.Path, pathlib.Path] = dataclasses.field(
        default_factory=dict
    )

    def merge_with(self, other: PartialIDK) -> MergedIDK:
        """Merge the contents of this MergedIDK with a PartialIDK and return the
        result.

        Put enough of them together, and you get a full IDK!
        """
        result = MergedIDK(
            atoms=_merge_atoms(self.atoms, other.atoms),
            dest_to_src=self.dest_to_src,
        )

        for atom in other.atoms.values():
            result.dest_to_src = _merge_other_files(
                result.dest_to_src, atom.dest_to_src
            )
        return result

    def sdk_manifest_json(
        self, host_arch: str, target_arch: list[str], release_version: str
    ) -> Any:
        """Returns the contents of manifest.json to include in the IDK."""
        index = []
        for meta_path, atom in self.atoms.items():
            type = get_idk_manifest_type_file_for_atom_type(atom["type"])

            if type in TYPES_SUPPORTING_UNSTABLE_ATOMS:
                is_stable = atom["stable"]
            else:
                assert "stable" not in atom.keys()
                is_stable = True

            index.append(
                dict(
                    meta=str(meta_path),
                    type=type,
                    stable=is_stable,
                )
            )

        index.sort(key=lambda a: (a["meta"], a["type"]))

        return {
            "arch": {
                "host": host_arch,
                "target": target_arch,
            },
            "id": release_version,
            "parts": index,
            "root": "..",
            "schema_version": "1",
        }


def _merge_atoms(
    a: dict[pathlib.Path, AtomMeta], b: dict[pathlib.Path, PartialAtom]
) -> dict[pathlib.Path, AtomMeta]:
    """Merge two dictionaries full of atoms."""
    result = {}

    all_atoms = set([*a.keys(), *b.keys()])
    for atom_path in all_atoms:
        atom_a = a.get(atom_path)
        atom_b = b.get(atom_path)

        if atom_a and atom_b:
            # Merge atoms found in both IDKs.
            try:
                result[atom_path] = _merge_atom_meta(atom_a, atom_b.meta)
            except Exception as e:
                raise AtomMergeError(atom_path) from e
        elif atom_a:
            result[atom_path] = atom_a
        else:
            assert atom_b, "unreachable. Atom '%s' had falsy value?" % atom_path
            result[atom_path] = atom_b.meta
    return result


def _merge_other_files(
    a: dict[pathlib.Path, pathlib.Path],
    b: dict[pathlib.Path, pathlib.Path],
) -> dict[pathlib.Path, pathlib.Path]:
    """Merge two dictionaries from (destination -> src). Shared keys are only
    allowed if the value files have the same contents."""
    result = {}

    all_files = set([*a.keys(), *b.keys()])
    for dest in all_files:
        src_a = a.get(dest)
        src_b = b.get(dest)
        if src_a and src_b:
            # Unfortunately, sometimes two separate subbuilds provide the same
            # destination file (particularly, blobs within packages). We have to
            # support this, but make sure that the file contents are identical.

            # Only inspect the files if the paths differ. This way we don't need
            # to go to disk in all the tests.
            if src_a != src_b:
                assert filecmp.cmp(src_a, src_b, shallow=False), (
                    "Multiple non-identical files want to be written to %s:\n- %s\n- %s"
                    % (
                        dest,
                        src_a,
                        src_b,
                    )
                )
            result[dest] = src_a
        elif src_a:
            result[dest] = src_a
        else:
            assert src_b, "unreachable. File '%s' had falsy source?" % dest
            result[dest] = src_b

    return result


def _assert_dicts_equal(
    a: Mapping[str, Any], b: Mapping[str, Any], ignore_keys: list[str]
) -> None:
    """Assert that the given dictionaries are equal on all keys not listed in
    `ignore_keys`."""
    keys_to_compare = set([*a.keys(), *b.keys()]) - set(ignore_keys)
    for key in keys_to_compare:
        assert a.get(key) == b.get(
            key
        ), "Key '%s' does not match. a[%s] = %s; b[%s] = %s" % (
            key,
            key,
            a.get(key),
            key,
            b.get(key),
        )


T = TypeVar("T")
K = TypeVar("K")


def _merge_unique_variants(
    vs1: Sequence[T] | None,
    vs2: Sequence[T] | None,
    dedup_key: Callable[[T], K],
) -> list[T]:
    """Merge vs1 and vs2, and assert that all values are all unique when
    projected through `dedup_key`. If either argument is None, it is treated as
    if it was empty."""

    result = [*(vs1 or []), *(vs2 or [])]
    # For all pairs...
    for v1, v2 in itertools.combinations(result, 2):
        assert dedup_key(v1) != dedup_key(
            v2
        ), "found duplicate variants:\n- %s\n- %s" % (v1, v2)
    return result


def _merge_disjoint_dicts(
    a: dict[str, Any] | None, b: dict[str, Any] | None
) -> dict[str, Any]:
    """Merge two dicts, asserting that they have no overlapping keys. If either
    dict is None, it is treated as if it was empty."""
    if a and b:
        assert a.keys().isdisjoint(
            b.keys()
        ), "a and b have overlapping keys: %s vs %s" % (
            a.keys(),
            b.keys(),
        )
        return {**a, **b}
    else:
        return a or b or {}


def _merge_atom_meta(a: AtomMeta, b: AtomMeta) -> AtomMeta:
    """Merge two atoms, according to type-specific rules."""
    if a["type"] in get_args(get_type_hints(UnmergeableMeta)["type"]):
        _assert_dicts_equal(a, b, [])
        return a

    if a["type"] == "cc_prebuilt_library":
        # This needs to go in each case to appease the type checker.
        assert a["type"] == b["type"]

        # "binaries" contains the legacy API level unaware prebuilts, and
        # "variants" contains the API level-specific prebuilts. They are
        # mutually exclusive for a given [sub-]build.
        # The root "ifs" file corresponds to "binaries" and thus does not
        # exist in atoms with "variants".
        _assert_dicts_equal(a, b, ["binaries", "variants", "ifs"])
        a["binaries"] = _merge_disjoint_dicts(
            a.get("binaries"), b.get("binaries")
        )
        a["variants"] = _merge_unique_variants(
            a.get("variants"),
            b.get("variants"),
            lambda v: v["constraints"],
        )
        return a

    if a["type"] == "loadable_module":
        assert a["type"] == b["type"]
        _assert_dicts_equal(a, b, ["binaries"])
        a["binaries"] = _merge_disjoint_dicts(
            a.get("binaries"), b.get("binaries")
        )
        return a

    if a["type"] == "package":
        assert a["type"] == b["type"]
        _assert_dicts_equal(a, b, ["variants"])
        a["variants"] = _merge_unique_variants(
            a.get("variants"),
            b.get("variants"),
            lambda v: (v["api_level"], v["arch"]),
        )
        return a

    if a["type"] == "sysroot":
        assert a["type"] == b["type"]
        _assert_dicts_equal(a, b, ["versions", "variants"])
        a["versions"] = _merge_disjoint_dicts(
            a.get("versions"), b.get("versions")
        )
        a["variants"] = _merge_unique_variants(
            a.get("variants"), b.get("variants"), lambda v: v["constraints"]
        )
        return a

    raise AssertionError("Unknown atom type: " + a["type"])
