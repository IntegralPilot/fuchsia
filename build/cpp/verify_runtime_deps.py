#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import os
import typing as T

# The following runtime libraries are provided directly by the SDK sysroot,
# and not as SDK atoms.
_SYSROOT_LIBS = ["libc.so", "libzircon.so"]

# Information about an atom.
AtomInfo: T.TypeAlias = dict[str, T.Any]

# The contents of a JSON file.
JsonFileContent: T.TypeAlias = dict[str, T.Any]


_NON_SDK_DEPS_ERROR_HEADER = r"""## Non-SDK dependencies required at runtime:
"""

_NON_SDK_DEPS_ERROR_FOOTER = r"""
HINT: These should be defined by an sdk_shared_library() call, or by a
zx_library() one that sets 'sdk = "shared"'.
"""

_BAD_SDK_DEPS_ERROR_HEADER = r"""## Non prebuilt libraries required at runtime:
"""

_BAD_SDK_DEPS_ERROR_FOOTER = _NON_SDK_DEPS_ERROR_FOOTER

_MISSING_SDK_DEPS_ERROR_HEADER = r"""## No dependency generates SDK runtime requirement:
"""

_MISSING_SDK_DEPS_ERROR_FOOTER = r"""
HINT: Add the missing atom(s)'s targets to the 'runtime_deps' list.
"""


class DependencyErrors(object):
    """Models list of errors found during verification."""

    def __init__(self) -> None:
        self._errors: list[tuple[dict[str, str], str]] = []

    def has_error(self) -> bool:
        """Return True iff this instance contains errors."""
        return bool(self._errors)

    def add_non_sdk_dependency(self, entry: dict[str, str]) -> None:
        """Add an entry for a non-SDK dependency.

        This happens when an SDK atom depends on a shared_library() instance
        directly, instead of an skd_shared_library() one.
        """
        self._errors.append((entry, "non_sdk_deps"))

    def add_bad_sdk_dependency(self, entry: dict[str, str]) -> None:
        """Add an entry for a non-prebuilt shared library."""
        self._errors.append((entry, "bad_sdk_deps"))

    def add_missing_sdk_dependency(self, entry: dict[str, str]) -> None:
        """Add an entry for a non-SDK shared library dependency."""
        self._errors.append((entry, "missing_sdk_deps"))

    def __str__(self) -> str:
        result = ""
        # Split errors by categories
        errors = collections.defaultdict(list)
        for entry, category in self._errors:
            errors[category].append(entry)

        for category, entries in errors.items():
            if category == "non_sdk_deps":
                result += _NON_SDK_DEPS_ERROR_HEADER
                for entry in entries:
                    result += "- `%s` generated_by `%s`\n" % (
                        entry["source"],
                        entry["label"],
                    )
                result += _NON_SDK_DEPS_ERROR_FOOTER

            elif category == "bad_sdk_deps":
                result += _BAD_SDK_DEPS_ERROR_HEADER
                for entry in entries:
                    result += "- IDK atom `%s` generated_by `%s`\n" % (
                        entry["sdk_id"],
                        entry["label"],
                    )
                result += _BAD_SDK_DEPS_ERROR_FOOTER

            elif category == "missing_sdk_deps":
                result += _MISSING_SDK_DEPS_ERROR_HEADER
                for entry in entries:
                    result += "- IDK atom `%s` generated_by `%s`\n" % (
                        entry["sdk_id"],
                        entry["label"],
                    )
                result += _MISSING_SDK_DEPS_ERROR_FOOTER

            else:
                assert False, "Unknown category: %s" % category

        return result


def check_for_missing_runtime_deps(
    runtime_files: T.Sequence[dict[str, str]], atom_deps: dict[str, AtomInfo]
) -> DependencyErrors:
    """Verifies the runtime dependencies of a prebuilt IDK library (static or shared).

    Verifies the atoms corresponding to the list of runtime requirements in
    `runtime_files` are all included in `atom_deps`.

    This ensures that atoms containing all runtime files are included in the
    atom's deps so that IDK consumers will know to package them when using
    the atom. This also verifies those atoms will be included in the IDK.

    Args:
      runtime_files: A JSON list of dictionaries, each representing a runtime
        dependency. Each dictionary must follow the schema described in
        //build/cpp/verify_runtime_deps.gni and contain either an 'sdk_id' or a
        'source' key.
      atom_deps: A dictionary mapping the atom IDs of the atom and its
        dependencies to their corresponding atom information. (The
        implementation does not care whether the dependencies are only direct or
        include all its transitive dependencies, but the results may vary
        depending on this.)

    Returns:
      A DependencyErrors instance containing any errors found during
      verification.
    """
    errors = DependencyErrors()

    for entry in runtime_files:
        sdk_id = entry.get("sdk_id")
        source = entry.get("source")

        assert bool(sdk_id) != bool(source), (
            'Exactly one of "sdk_id" or "source" must be defined in entry: %s'
            % entry
        )

        if sdk_id:
            # This runtime dependency is an IDK library. Verify that the library
            # is in the atom's deps.
            assert not source, "Only one of `sdk_id` or `source` should be set."
            dep = atom_deps.get(sdk_id)
            if not dep:
                errors.add_missing_sdk_dependency(entry)
            elif "type" in dep:
                # The atom info is from the old .sdk build manifest.
                if dep["type"] != "cc_prebuilt_library":
                    errors.add_bad_sdk_dependency(entry)
            # Else the atom info is from prebuild info.
            elif dep["atom_type"] != "cc_prebuilt_library":
                errors.add_bad_sdk_dependency(entry)
        elif source:
            # A non-IDK library.

            # Ignore sysroot libs
            if os.path.basename(source) in _SYSROOT_LIBS:
                # TODO(https://fxbug.dev/447151364): Determine whether this
                # unused logic is necessary.
                assert False
                continue

            # This runtime dependency is *not* an IDK library. This is an error.
            errors.add_non_sdk_dependency(entry)
        else:
            assert False, "Runtime entry is missing 'sdk_id' or 'source'."

    return errors
