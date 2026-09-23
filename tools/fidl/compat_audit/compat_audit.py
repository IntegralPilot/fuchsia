#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from importlib import resources
from pathlib import Path
from typing import Any, Dict, List

# Hard-coded list mapping API area to bug ID for the audit.
BUG_IDS = {
    "Bluetooth": 467127725,
    "Component Framework": 467128242,
    "Developer": 467127620,
    "Diagnostics": 467128231,
    "Driver SDK": 467128685,
    "Drivers": 467128430,
    "Experiences": 467128325,
    "Graphics": 467127396,
    "Kernel": 467127420,
    "Media": 467128527,
    "Metrics": 467128803,
    "Netstack": 467128725,
    "Power": 467127481,
    "Software Delivery": 467128508,
    "Storage": 467128288,
    "Testing": 467128984,
    "UI": 467128273,
    "Unknown": 467128885,
    "WLAN": 467128644,
    "Web": 467127821,
}


@dataclass
class Location:
    filename: str
    line: int
    column: int
    length: int

    @staticmethod
    def from_json_value(json_val: Dict[str, Any]) -> "Location":
        return Location(
            filename=json_val["filename"],
            line=json_val["line"],
            column=json_val["column"],
            length=json_val["length"],
        )


@dataclass
class Value:
    value: str

    @staticmethod
    def from_json_value(json_val: Dict[str, Any]) -> "Value":
        return Value(value=json_val["value"])


@dataclass
class Argument:
    name: str
    value: Value

    @staticmethod
    def from_json_value(json_val: Dict[str, Any]) -> "Argument":
        return Argument(
            name=json_val["name"],
            value=Value.from_json_value(json_val["value"]),
        )


@dataclass
class Attribute:
    name: str
    arguments: List[Argument] = field(default_factory=list)

    @staticmethod
    def from_json_value(json_val: Dict[str, Any]) -> "Attribute":
        return Attribute(
            name=json_val["name"],
            arguments=[
                Argument.from_json_value(arg)
                for arg in json_val.get("arguments", [])
            ],
        )


@dataclass
class Declaration:
    name: str
    strict: bool
    deprecated: bool
    is_result: bool
    location: Location
    sdk_area: str
    maybe_attributes: List[Attribute] = field(default_factory=list)

    @staticmethod
    def from_json_value(json_val: Dict[str, Any]) -> "Declaration":
        return Declaration(
            name=json_val["name"],
            strict=json_val["strict"],
            deprecated=json_val["deprecated"],
            # Use .get because not all decls have this field.
            is_result=json_val.get("is_result", False),
            location=Location.from_json_value(json_val["location"]),
            sdk_area=json_val["metadata"]["sdk_area"],
            maybe_attributes=[
                Attribute.from_json_value(attr)
                for attr in json_val["maybe_attributes"]
            ],
        )

    def is_unstable(self) -> bool:
        for attr in self.maybe_attributes:
            if attr.name == "available":
                for arg in attr.arguments:
                    if arg.name == "added" and arg.value.value == "4292870144":
                        return True
        return False

    def already_audited(self) -> bool:
        for attr in self.maybe_attributes:
            if attr.name == "strict_audit":
                return True
        return False


# Types that support the `strict` keyword.
STRICT_TYPES = [
    "bits_declarations",
    "enum_declarations",
    "union_declarations",
]


def get_all_decls() -> List[Declaration]:
    """Get all declarations from the compiled-in platform IR."""
    ir_path = resources.files("compat_audit_data").joinpath(
        "platform-ir-HEAD.json"
    )
    ir_content = ir_path.read_text()
    data = json.loads(ir_content)

    all_decls = []
    for kind in STRICT_TYPES:
        for decl_json in data[kind]:
            # Skip zx.* decls. Their versioning constraints are different.
            if decl_json["name"].startswith("zx/"):
                continue
            all_decls.append(Declaration.from_json_value(decl_json))
    return all_decls


def get_decls_to_audit() -> List[Declaration]:
    """Get all declarations that need to be audited."""
    all_decls = get_all_decls()
    decls_to_audit = []
    for decl in all_decls:
        if (
            decl.strict
            and not decl.deprecated
            and not decl.is_unstable()
            # Results with the error syntax are treated as strict enums, but they're not relevant
            # here.
            and not decl.is_result
            and not decl.already_audited()
        ):
            decls_to_audit.append(decl)
    return decls_to_audit


def run_add_attributes(args: argparse.Namespace) -> int:
    decls = get_decls_to_audit()

    if args.dry_run:
        print("[DRY RUN] Declarations to be updated:")
        for decl in decls:
            print(decl.name)
        return 0

    edits_by_file = defaultdict(list)
    for decl in decls:
        loc = decl.location
        rel_path = loc.filename
        while rel_path.startswith("../"):
            rel_path = rel_path[3:]
        file_path = (args.fuchsia_root / rel_path).resolve()
        edits_by_file[file_path].append(decl)

    for file_path, locations in edits_by_file.items():
        with open(file_path, "r") as f:
            lines = f.readlines()

        # Group declarations by line number
        line_modifications = defaultdict(list)
        for decl in locations:
            line_modifications[decl.location.line].append(decl)

        modified = False
        # Process lines in order
        for line_num in sorted(line_modifications.keys()):
            index = line_num - 1
            if not (0 <= index < len(lines)):
                raise RuntimeError(
                    f"Line number {line_num} out of bounds for {file_path.name}"
                )

            current_line = lines[index]

            # Sort declarations for this line by column, DESCENDING to handle offsets correctly
            decls_for_line = sorted(
                line_modifications[line_num],
                key=lambda decl: decl.location.column,
                reverse=True,
            )

            for decl in decls_for_line:
                loc = decl.location
                col = loc.column - 1  # FIDL is 1-based, string index is 0-based

                # Special case: If preceded by "type ", insert before it.
                if col >= 5 and current_line[col - 5 : col] == "type ":
                    col -= 5

                bug_id = BUG_IDS[decl.sdk_area]
                attribute_to_add = f'@strict_audit(bug="https://fxbug.dev/{bug_id}", state="PENDING_REVIEW")'

                if 0 <= col <= len(current_line):
                    current_line = (
                        current_line[:col]
                        + attribute_to_add
                        + " "
                        + current_line[col:]
                    )
                    modified = True
                else:
                    raise RuntimeError(
                        f"Column number {loc.column} out of bounds for line {line_num} in {file_path.name}"
                    )
            lines[index] = current_line

        if modified:
            with open(file_path, "w") as f:
                f.writelines(lines)

    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Tools for FIDL API/ABI evolvability auditing."
    )
    subparsers = parser.add_subparsers(dest="command", help="Subcommand to run")
    subparsers.required = True

    # Add Attributes Subcommand
    add_parser = subparsers.add_parser(
        "add-attributes", help="Add audit attributes to FIDL files."
    )
    add_parser.add_argument(
        "--fuchsia-root",
        type=Path,
        required=True,
        help="Root directory of the Fuchsia checkout",
    )
    add_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the changes that would be made without modifying files",
    )
    add_parser.set_defaults(func=run_add_attributes)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
