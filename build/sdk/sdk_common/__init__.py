# Copyright 2018 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import difflib
import functools
import json
import pathlib
from collections.abc import Iterator, Sequence
from typing import Any


class File:
    """Wrapper class for file definitions."""

    def __init__(self, json: dict[str, Any]) -> None:
        self.source: str = json["source"]
        self.destination: str = json["destination"]

    def __str__(self) -> str:
        return "{%s <-- %s}" % (self.destination, self.source)


@functools.total_ordering
class MinimalAtom(object):
    """Wrapper class for minimal atom data, adding convenience methods."""

    def __init__(self, json: dict[str, Any]) -> None:
        self.id: str = json["id"]
        self.label: str = json["gn-label"]
        self.category: str = json["category"]
        self.type: str = json["type"]
        self.area: str | None = json.get("area")

    @classmethod
    def from_values(
        cls,
        id: str,
        label: str,
        category: str,
        type: str,
        area: str | None = None,
    ) -> "MinimalAtom":
        json_data = {
            "id": id,
            "gn-label": label,
            "category": category,
            "type": type,
        }
        if area:
            json_data["area"] = area
        return cls(json_data)

    def __str__(self) -> str:
        return str(self.id)

    def __hash__(self) -> int:
        return hash(self.label)

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, MinimalAtom):
            return False
        return self.label == other.label

    def __ne__(self, other: Any) -> bool:
        if not isinstance(other, MinimalAtom):
            return True
        return not self.__eq__(other)

    def __lt__(self, other: Any) -> bool:
        if not isinstance(other, MinimalAtom):
            return False
        return self.id < other.id


@functools.total_ordering
class Atom(MinimalAtom):
    """Wrapper class for atom data, adding convenience methods."""

    def __init__(self, json: dict[str, Any]) -> None:
        super().__init__(json)
        self.json = json
        self.metadata: str = json["meta"]
        self.deps: Sequence[str] = json["deps"]
        self.files: Sequence[File] = [File(f) for f in json["files"]]
        self.stable: bool = json["stable"]

    def __hash__(self) -> int:
        return super().__hash__()

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, Atom):
            return False
        return self.label == other.label

    def __ne__(self, other: Any) -> bool:
        if not isinstance(other, Atom):
            return True
        return not self.__eq__(other)

    def __lt__(self, other: Any) -> bool:
        if not isinstance(other, Atom):
            return False
        return self.id < other.id


def detect_collisions(atoms: Sequence[MinimalAtom]) -> Iterator[str]:
    """Detects name collisions in a given atom list. Yields a series of error
    messages as strings."""
    mappings = collections.defaultdict(lambda: [])
    for atom in atoms:
        mappings[atom.id].append(atom)
    for id, group in mappings.items():
        if len(group) == 1:
            continue
        labels = [a.label for a in group]
        msg = "Targets sharing the SDK id %s:\n" % id
        for label in labels:
            msg += " - %s\n" % label
        yield msg


CATEGORIES = [
    "internal",
    "compat_test",
    "host_tool",
    "prebuilt",
    "partner",
]


def _index_for_category(category: str) -> int:
    if not category in CATEGORIES:
        raise Exception('Unknown SDK category "%s"' % category)
    return CATEGORIES.index(category)


def detect_category_violations(
    category: str, atoms: Sequence[MinimalAtom]
) -> Iterator[str]:
    """Yields strings describing mismatches in publication categories."""
    category_index = _index_for_category(category)
    for atom in atoms:
        if _index_for_category(atom.category) < category_index:
            yield (
                '"%s" has publication level "%s", which is incompatible with "%s".'
                % (atom, atom.category, category)
            )


def area_names_from_file(parsed_areas: Any) -> list[str]:
    """Given a parsed version of docs/contribute/governance/areas/_areas.yaml,
    return a list of acceptable area names."""
    return [area["name"] for area in parsed_areas] + ["Unknown"]


_VALID_ATOM_TYPES = [
    # LINT.IfChange(idk_atom_types)
    "bind_library",
    "cc_prebuilt_library",
    "cc_source_library",
    "companion_host_tool",
    "dart_library",
    "data",
    "documentation",
    "experimental_python_e2e_test",
    "fidl_library",
    "ffx_tool",
    "host_tool",
    "loadable_module",
    "package",
    "sysroot",
    "version_history",
    # LINT.ThenChange(//build/bazel/bazel_idk/private/idk_atom.bzl:idk_atom_types, //build/sdk/generate_prebuild_idk/idk_generator.py, //build/sdk/manifest_schema.json, //build/sdk/meta/BUILD.bazel:schema_in_idk, //build/sdk/meta/BUILD.gn:schema_in_idk)
]
# IdkGenerator may pass "none" as well, but it should not be included in the
# list of valid types above.
_VALID_ATOM_TYPES_PLUS_NONE = _VALID_ATOM_TYPES + [
    "none",
]

# Remove the types requiring area from the list of all types to get the types
# for which area is optional.
_AREA_OPTIONAL_TYPES = _VALID_ATOM_TYPES_PLUS_NONE[:]
_AREA_OPTIONAL_TYPES.remove("bind_library")
_AREA_OPTIONAL_TYPES.remove("fidl_library")


class Validator:
    """Helper class to validate sets of IDK atoms."""

    def __init__(self, valid_areas: Sequence[str]) -> None:
        """Construct a validator with a given set of areas. Exposed for
        testing. Use Validator.from_areas_file_path instead."""
        self._valid_areas = valid_areas

    @classmethod
    def from_areas_file_path(cls, areas_file: pathlib.Path) -> "Validator":
        """Build a Validator given a path to
        docs/contribute/governance/areas/_areas.yaml."""
        import yaml

        with areas_file.open() as f:
            parsed_areas = yaml.safe_load(f)
            return Validator(area_names_from_file(parsed_areas))

    def detect_violations(
        self, category: str | None, atoms: Sequence[MinimalAtom]
    ) -> Iterator[str]:
        """Yield strings describing all violations found in `atoms`."""
        yield from detect_collisions(atoms)
        if category:
            yield from detect_category_violations(category, atoms)
        yield from self.detect_invalid_types(atoms)
        yield from self.detect_area_violations(atoms)

    def detect_invalid_types(
        self, atoms: Sequence[MinimalAtom]
    ) -> Iterator[str]:
        """Yields strings describing any invalid types in `atoms`."""
        for atom in atoms:
            if atom.type not in _VALID_ATOM_TYPES_PLUS_NONE:
                yield (
                    "Atom type `%s` for `%s` is unsupported. Valid types are: %s"
                    % (
                        atom.type,
                        atom,
                        sorted(_VALID_ATOM_TYPES),
                    )
                )

    def detect_area_violations(
        self, atoms: Sequence[MinimalAtom]
    ) -> Iterator[str]:
        """Yields strings describing any invalid API areas in `atoms`."""
        for atom in atoms:
            if atom.area is None and atom.type not in _AREA_OPTIONAL_TYPES:
                yield (
                    "%s must specify an API area. Valid areas: %s"
                    % (
                        atom,
                        self._valid_areas,
                    )
                )

            if atom.area is not None and atom.area not in self._valid_areas:
                if matches := difflib.get_close_matches(
                    atom.area, self._valid_areas
                ):
                    yield (
                        "%s specifies invalid API area '%s'. Did you mean one of these? %s"
                        % (atom, atom.area, matches)
                    )
                else:
                    yield (
                        "%s specifies invalid API area '%s'. Valid areas: %s"
                        % (atom, atom.area, self._valid_areas)
                    )
