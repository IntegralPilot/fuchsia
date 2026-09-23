#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Android misc_info.txt utility.

Parses an Android `misc_info.txt` file and extracts vbmeta property descriptors.
"""

import argparse
import json
import pathlib


def _misc_info_to_dict(content: str) -> dict[str, str]:
    """Parses misc_info.txt contents into key-value pairs.

    Processes all lines that have KEY=VALUE format.
    """
    misc_info: dict[str, str] = {}
    for line in content.splitlines():
        if "=" in line:
            key, val = line.split("=", maxsplit=1)
            misc_info[key.strip()] = val.strip()
    return misc_info


def _extract_vbmeta_properties(key: str, value: str) -> dict[str, str]:
    """Extracts vbmeta property name/value pairs from a misc_info.txt key/value.

    Args:
        key: a misc_info.txt key
        value: a misc_info.txt value

    Returns:
        A {prop_name: prop_value} dict, or the empty dict if `key` doesn't look like a
        vbmeta commandline.
    """
    # Example:
    #   key: "avb_boot_add_hash_footer_args"
    #   value: "--prop foo:10 --prop bar:ABC --rollback_index 1000"
    if not (key.startswith("avb_") and key.endswith("_args")):
        return {}

    # misc_info.txt doesn't quote or escape, so we don't handle it for simplicity
    # but want to be alerted if we ever do need to start handling it.
    if any(c in value for c in ['"', "'", "\\"]):
        raise NotImplementedError(f"Unsupported meta-char in '{value}'")

    # Use argparse to extract just the `--prop` args we care about.
    parser = argparse.ArgumentParser(allow_abbrev=False, exit_on_error=False)
    parser.add_argument("--prop", action="append", default=[])
    try:
        parsed_args, _ = parser.parse_known_args(value.split())
    except argparse.ArgumentError as e:
        raise ValueError(f"Failed to parse arguments: {e}") from e

    props: dict[str, str] = {}
    for prop in parsed_args.prop:
        if ":" not in prop:
            raise ValueError(
                f"Invalid property format '{prop}', expected 'KEY:VALUE'"
            )
        prop_key, prop_val = prop.split(":", 1)
        props[prop_key] = prop_val

    return props


def process_misc_info(content: str) -> dict[str, str]:
    """Parses misc_info.txt and extracts vbmeta properties as a {name: value} dict."""
    misc_info_entries = _misc_info_to_dict(content)

    all_props: dict[str, str] = {}
    for key, value in misc_info_entries.items():
        new_props = _extract_vbmeta_properties(key, value)
        # We combine all properties into a single flat dict, discarding the specific
        # vbmeta blob the property was written into. Make sure each property is unique
        # so we don't silently clobber a property from one vbmeta image with a property
        # from another.
        overlap = all_props.keys() & new_props.keys()
        if overlap:
            raise ValueError(f"Duplicate properties found: {overlap}")
        all_props |= new_props

    return all_props


def _write_output(
    props: dict[str, str],
    format: str = "json",
    output: pathlib.Path | None = None,
) -> None:
    """Writes extracted properties to a file or stdout in the requested format.

    Args:
        props: vbmeta properties to write.
        format: output format, either "json" or "raw_value".
        output: output file path, or None to print to stdout.
    """
    if format == "raw_value":
        # Since raw_value just outputs a value, it doesn't work with multiple
        # properties as it would lose the property name mapping.
        if len(props) != 1:
            raise ValueError(
                "raw_value is only supported for a single property"
            )
        output_str = list(props.values())[0]
    elif format == "json":
        output_str = json.dumps(props, indent=2, sort_keys=True)
    else:
        raise ValueError(f"Unsupported format: {format}")

    if output:
        output.write_text(output_str, encoding="utf-8")
    else:
        print(output_str)


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Parses command-line arguments."""
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "misc_info",
        type=pathlib.Path,
        help="Path to misc_info.txt file",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=pathlib.Path,
        help="Path to output file (defaults to stdout)",
    )
    parser.add_argument(
        "--prop",
        action="append",
        default=[],
        help="If provided, only output the given properties (may be passed "
        "multiple times). Use NAME:NEW_NAME to also rename the output property",
    )
    # Supported formats:
    #
    # JSON is most useful for the build system, for example creating Structured
    # Configuration values from a misc_info.txt. Also useful for general-purpose
    # viewing or chaining into a different script.
    #
    # Raw value format is specifically for avbtool's `--prop_from_file` argument,
    # which takes in a file containing just the raw property value. This can be used
    # to generate our own vbmeta images containing properties extracted from a
    # misc_info.txt.
    parser.add_argument(
        "--format",
        choices=["json", "raw_value"],
        default="json",
        help="Output format (default: %(default)s). raw_value is only possible when "
        "the output is a single property",
    )

    args = parser.parse_args(argv)

    # Reformat any selected --prop(s) into a more Python-friendly dict containing
    # {name: new_name}. If renaming isn't requested, `name` == `new_name`.
    prop_map = {}
    for prop_arg in args.prop:
        parts = prop_arg.split(":", maxsplit=1)
        name = parts[0]
        new_name = parts[1] if len(parts) > 1 else name
        prop_map[name] = new_name
    args.prop = prop_map or None

    return args


def main(argv: list[str] | None = None) -> None:
    args = _parse_args(argv)

    props = process_misc_info(args.misc_info.read_text(encoding="utf-8"))

    # If we got any --prop args, filter to just those properties, potentially
    # also renaming them if requested.
    if args.prop:
        props = {new_name: props[name] for name, new_name in args.prop.items()}

    _write_output(props, format=args.format, output=args.output)


if __name__ == "__main__":
    main()
