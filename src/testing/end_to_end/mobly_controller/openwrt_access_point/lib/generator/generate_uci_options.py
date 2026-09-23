#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generates Python profile classes from OpenWrt JSON schemas.

This script fetches the JSON schema for OpenWrt configuration and generates
a Python dataclass containing a subset of properties (specified in an allow-list).

It will read the default configuration (git tag and attributes) from:
    uci_allow_list.yaml
"""

import json
import os
import sys
import urllib.request
from typing import Any

import yaml

# ANSI Colors for output
GREEN = "\033[0;32m"
RED = "\033[0;31m"
YELLOW = "\033[0;33m"
BLUE = "\033[0;34m"
RESET = "\033[0m"


def print_error(msg: str):
    print(f"{RED}Error: {msg}{RESET}", file=sys.stderr)


def print_success(msg: str):
    print(f"{GREEN}{msg}{RESET}")


def fetch_schema(tag: str, schema_name: str) -> dict[str, Any]:
    """Fetches the JSON schema from OpenWrt GitHub."""
    url = (
        f"https://raw.githubusercontent.com/openwrt/openwrt/{tag}/"
        f"package/network/config/wifi-scripts/files-ucode/usr/share/schema/{schema_name}"
    )
    print(f"Fetching schema from: {url}", file=sys.stderr)

    try:
        with urllib.request.urlopen(url) as response:
            if response.status != 200:
                raise Exception(f"HTTP error: {response.status}")
            return json.loads(response.read().decode("utf-8"))
    except Exception as e:
        print(f"Failed to fetch schema: {e}", file=sys.stderr)
        sys.exit(1)


def map_type(prop_info: dict[str, Any], prop_name: str = "") -> str:
    """Maps JSON schema types to Python types."""

    json_type = prop_info.get("type", "string")

    if json_type == "array":
        items = prop_info.get("items")
        if items is None:
            raise ValueError(
                f"Array property '{prop_name}' is missing 'items' definition."
            )

        item_type = items.get("type")
        if item_type is None:
            raise ValueError(
                f"Array items for property '{prop_name}' is missing 'type' definition."
            )

        inner_type = map_type({"type": item_type}, "")
        return f"list[{inner_type}]"

    mapping = {
        "string": "str",
        "boolean": "bool",
        "number": "int",
    }
    if json_type not in mapping:
        raise ValueError(
            f"Unsupported JSON type '{json_type}' for property '{prop_name}'."
        )
    return mapping[json_type]


def _process_property(
    prop_name: str, properties: dict[str, Any], resolved_names: dict[str, str]
) -> list[str]:
    """Processes a single property and returns the generated lines."""

    def get_prop(name: str) -> tuple[dict, str]:
        prop_info = properties.get(name)
        if not prop_info:
            raise ValueError(f"Property '{name}' not found in schema.")

        json_type = prop_info.get("type")
        if json_type is None:
            raise ValueError(
                f"Property '{name}' is missing 'type' definition in schema."
            )
        return prop_info, json_type

    prop_info, json_type = get_prop(prop_name)
    target_name = prop_name

    # Resolve schema-defined aliases to their canonical name
    if json_type == "alias":
        target_name = prop_info.get("default")
        if target_name is None:
            raise ValueError(
                f"Alias property '{prop_name}' is missing 'default' field specifying the canonical name."
            )
        prop_info, json_type = get_prop(target_name)

    if target_name in resolved_names:
        raise ValueError(
            f"'{prop_name}' and '{resolved_names[target_name]}' are duplicates of '{target_name}'"
        )
    resolved_names[target_name] = prop_name

    python_type = map_type(prop_info, prop_name)

    enum_values = prop_info.get("enum")
    if enum_values:
        formatted_values = []
        for val in enum_values:
            if isinstance(val, str):
                formatted_values.append(f'"{val}"')
            else:
                formatted_values.append(str(val))
        python_type = f"Literal[{', '.join(formatted_values)}]"

    description = prop_info.get("description", "").replace("\n", " ")

    lines = []
    lines.append("")
    lines.append(f"    {prop_name}: {python_type}")
    if description:
        lines.append(f'    """{description}"""')

    return lines


def generate_code(
    schema: dict[str, Any],
    allow_list: list[str],
    class_name: str,
    schema_name: str,
    tag: str,
) -> str:
    """Generates the Python dataclass code."""
    properties = schema.get("properties", {})
    resolved_names = {}  # target_name -> original_requested_name

    prop_lines_all = []
    uses_literal = False
    errors = []

    for prop_name in allow_list:
        try:
            prop_lines = _process_property(
                prop_name, properties, resolved_names
            )
            prop_lines_all.extend(prop_lines)
            # Check if any generated line uses Literal
            if any("Literal[" in line for line in prop_lines):
                uses_literal = True
        except ValueError as e:
            errors.append(str(e))

    if errors:
        print(f"\n{RED}Found {len(errors)} error(s):{RESET}", file=sys.stderr)
        for err in errors:
            print_error(err)
        sys.exit(1)

    lines = []
    lines.append("# Copyright 2026 The Fuchsia Authors. All rights reserved.")
    lines.append(
        "# Use of this source code is governed by a BSD-style license that can be"
    )
    lines.append("# found in the LICENSE file.")
    lines.append("")
    lines.append("# THIS FILE IS GENERATED. DO NOT EDIT MANUALLY.")
    lines.append("# To update, edit uci_allow_list.yaml and run:")
    lines.append(
        "#   python3 src/testing/end_to_end/mobly_controller/openwrt_access_point/lib/generator/generate_uci_options.py"
    )
    lines.append(f"# Schema: {schema_name}")
    lines.append(f"# Tag: {tag}")
    lines.append("")

    imports = ["TypedDict"]
    if uses_literal:
        imports.append("Literal")
    lines.append(f"from typing import {', '.join(sorted(imports))}")
    lines.append("")
    lines.append("")
    lines.append(f"class {class_name}(TypedDict, total=False):")
    lines.append('    """Generated from OpenWrt JSON schema.')
    lines.append("")
    lines.append("    Only includes attributes specified in the allow-list.")
    lines.append('    """')

    lines.extend(prop_lines_all)
    lines.append("")

    return "\n".join(lines)


def main() -> None:
    script_dir = os.path.dirname(os.path.abspath(__file__))
    config_path = os.path.join(script_dir, "uci_allow_list.yaml")

    config = {}
    if os.path.exists(config_path):
        with open(config_path, "r") as f:
            loaded = yaml.safe_load(f)
            if isinstance(loaded, dict):
                config = loaded

    git_tag = config.get("git_tag")
    if git_tag is None:
        print_error("'git_tag' not found in config file.")
        sys.exit(1)
    targets = config.get("targets", [])

    for target in targets:
        schema_name = target.get("schema")
        class_name = target.get("class_name")
        output_rel_path = target.get("output")
        attributes = target.get("attributes", [])

        print(f"\n--- Processing target: {class_name} ---")
        schema = fetch_schema(git_tag, schema_name)
        code = generate_code(
            schema, attributes, class_name, schema_name, git_tag
        )

        output_path = os.path.abspath(os.path.join(script_dir, output_rel_path))

        with open(output_path, "w") as f:
            f.write(code)
        print(f"Wrote generated code to {output_path}")

    print_success("\nAll done! Successfully generated all options.")


if __name__ == "__main__":
    main()
