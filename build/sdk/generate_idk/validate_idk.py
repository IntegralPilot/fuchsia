# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Validates the contents of an IDK directory.

Currently, this is limited to validating the schema.

Usage:
    python3 ./build/sdk/generate_idk/validate_idk.py \
        --idk-directory $(fx get-build-dir)/sdk/exported/fuchsia_idk \
        --schema-directory build/sdk/meta \
        --json-validator-path $(fx get-build-dir)/host_x64/json_validator_valico
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

# Disallow bytecode generation to avoid polluting the Bazel execroot with .pyc
# files that can end up in the generated TreeArtifact, resulting in issues when
# dependent actions try to read it.
sys.dont_write_bytecode = True

# Atom types that are treated as type "data".
_VALID_DATA_ATOM_TYPES = [
    # LINT.IfChange
    "component_manifest",
    "config",
    # LINT.ThenChange(//build/sdk/sdk_atom.gni)
]


def get_idk_manifest_type_file_for_atom_type(type: str) -> str:
    """Returns the type string used in the IDK manifest for the atom `type`.

    Args:
        type: The atom type as used in an individual atom manifest.
    Returns:
        The corresponding type used in the IDK manifest.
    """
    if type in _VALID_DATA_ATOM_TYPES:
        return "data"
    else:
        return type


class AtomSchemaValidator:
    """Validates atom metadata against JSON schemas."""

    def __init__(self, schema_directory: Path, json_validator_path: Path):
        """Initializes the object.

        Args:
            schema_directory: Path to the directory containing the schema files
                for IDK contents.
            json_validator_path: Path to an executable that validates JSON
                against a schema.
        """
        self.schema_directory = schema_directory
        self.json_validator_path = json_validator_path

    def _get_schema_filename_for_atom_type(self, type: str) -> str:
        """Returns the filename of the schema file for the atom `type`.

        Args:
            type: The atom type as used in an individual atom manifest.
        Returns:
            The filename for the schema file used to validate `type`.
        """
        manifest_type = get_idk_manifest_type_file_for_atom_type(type)
        return f"{manifest_type}.json"

    def validate(self, file_path: Path, type: str) -> int:
        """Validates that `file_path` complies with the schema for the `type`.

        Deps on the schema files is covered by the collection's deps on
        # "//build/sdk/meta".

        Args:
            file_path: The metadata file to validate.
            type: The file's atom type as used in an individual atom manifest.
        Returns:
            0 if successful and non-zero otherwise.
        """
        schema_filename = self._get_schema_filename_for_atom_type(type)
        schema_file = self.schema_directory / schema_filename

        ret = subprocess.run([self.json_validator_path, schema_file, file_path])
        if ret.returncode != 0:
            print(
                f"ERROR: Metadata schema validation failed for '{os.path.abspath(file_path)}' of type '{type}' using schema '{schema_file}'."
            )
            return ret.returncode

        return 0


class IdkValidator:
    """Validates the contents of an IDK.

    Currently, this is limited to validating the schema.
    """

    def __init__(
        self, idk_dir: Path, schema_directory: Path, json_validator_path: Path
    ):
        """Initializes the object.

        Args:
            idk_dir: Root directory of the IDK to be verified. Must contain a
                file at `meta/manifest.json` referencing the manifests for all
                atoms.
            schema_directory: Path to the directory containing the schema files
                for IDK contents.
            json_validator_path: Path to an executable that validates JSON
                against a schema.
        """
        self._idk_dir: Path = idk_dir
        self._atom_schema_validator = AtomSchemaValidator(
            schema_directory, json_validator_path
        )

    def validate_schema(self) -> int:
        """Reads the IDK manifest and validates the schema of all manifests.

        Returns:
            0 if successful and non-zero otherwise.
        """
        _idk_manifest_path = self._idk_dir / "meta/manifest.json"

        with (_idk_manifest_path).open() as f:
            idk_manifest = json.load(f)

        # Do some basic checking.
        assert idk_manifest["arch"]["host"] != ""
        assert len(idk_manifest["parts"]) > 0
        assert (
            len(idk_manifest["arch"]["target"]) > 0
            and len(idk_manifest["arch"]["target"]) <= 3
        )
        assert idk_manifest["root"] == ".."
        assert idk_manifest["schema_version"] == "1"  # There is only one.

        result = self._atom_schema_validator.validate(
            _idk_manifest_path, "manifest"
        )
        if result != 0:
            return result

        atoms = idk_manifest["parts"]

        for atom in atoms:
            assert atom["stable"] == True or atom["stable"] == False
            meta_file_path = self._idk_dir / atom["meta"]
            atom_type = atom["type"]

            result = self._atom_schema_validator.validate(
                meta_file_path, atom_type
            )
            if result != 0:
                return result

        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)

    parser.add_argument(
        "--idk-directory",
        help="Root directory of the IDK to be verified. Must contain a file at `meta/manifest.json` referencing the manifests for all atoms.",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--schema-directory",
        type=Path,
        help="Path containing the metadata schema files",
        required=True,
    )
    parser.add_argument(
        "--json-validator-path",
        type=Path,
        help="Path containing the metadata schema files",
        required=True,
    )
    parser.add_argument(
        "--stamp-file",
        type=Path,
        help="Stamp file to write on success.",
        required=False,
    )

    args = parser.parse_args()

    idk_validator = IdkValidator(
        args.idk_directory, args.schema_directory, args.json_validator_path
    )
    result = idk_validator.validate_schema()
    if result != 0:
        return result

    if args.stamp_file:
        args.stamp_file.parent.mkdir(parents=True, exist_ok=True)
        args.stamp_file.write_text("OK")

    return 0


if __name__ == "__main__":
    sys.exit(main())
