# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import re
import sys

# Tokenizer for protobuf extension declarations. Group 'ignore' matches comments and
# string literals so they can be skipped. Group 'stmt' matches active extension declarations
# at the start of a line, with 'indent' capturing leading whitespace and 'body' capturing
# the comma-separated range items before the semicolon.
_EXTENSIONS_TOKEN_RE = re.compile(
    r'(?P<ignore>/\*.*?\*/|//[^\n]*|"[^"\\]*(?:\\.[^"\\]*)*"|\'[^\'\\]*(?:\\.[^\'\\]*)*\')|^(?P<indent>[ \t]*)(?P<stmt>extensions\s+(?P<body>[^;]+);)',
    flags=re.MULTILINE | re.DOTALL,
)

# Matches individual extension item strings, either a single field ID ("76") or a range
# ("70 to 80", "1000 to max"). Keywords 'to' and 'max' are strictly lowercase per protobuf spec.
_RANGE_RE = re.compile(r"^(\d+)(?:\s+to\s+(\d+|max))?$")

# Tokenizer for brace matching in protobuf files. Matches comments (both C-style /* ... */
# and C++-style // ...) and string literals in 'ignore' so they can be skipped, while
# capturing structural opening/closing braces in 'brace'.
_TOKEN_RE = re.compile(
    r'(?P<ignore>/\*.*?\*/|//[^\n]*|"[^"\\]*(?:\\.[^"\\]*)*"|\'[^\'\\]*(?:\\.[^\'\\]*)*\')|(?P<brace>[{}])',
    flags=re.DOTALL,
)

# Tokenizer for stripping comments while safely preserving string literals (e.g. URLs in options).
_COMMENT_TOKEN_RE = re.compile(
    r'(?P<ignore>/\*.*?\*/|//[^\n]*)|(?P<string>"[^"\\]*(?:\\.[^"\\]*)*"|\'[^\'\\]*(?:\\.[^\'\\]*)*\')',
    flags=re.DOTALL,
)


def _strip_comments(content: str) -> str:
    """Strips C-style and C++-style comments while safely preserving string literals."""
    return _COMMENT_TOKEN_RE.sub(
        lambda m: "" if m.lastgroup == "ignore" else m.group("string"),
        content,
    )


def _find_pattern_bounds(
    proto_content: str,
    pattern: re.Pattern[str] | str,
    block_name: str,
) -> tuple[int, int]:
    """Finds the start and closing brace index for a pattern in proto content.

    Args:
        proto_content: The full content of the proto file to search.
        pattern: A regex pattern or string matching the start of the block.
        block_name: A descriptive name of the block for error messages.

    Returns:
        A tuple of (start_index, closing_brace_index) of the matched block.

    Raises:
        ValueError: If the pattern, open brace, or matching closing brace is not found.
    """
    if isinstance(pattern, str):
        pattern = re.compile(pattern)
    match = pattern.search(proto_content)
    if not match:
        raise ValueError(f"{block_name} not found in proto")

    start_index = match.start()
    brace_offset = match.group(0).find("{")
    if brace_offset != -1:
        brace_index = start_index + brace_offset
    else:
        brace_index = proto_content.find("{", match.end())
    if brace_index == -1:
        raise ValueError(f"Open brace not found after {block_name}")

    brace_count = 1
    for m in _TOKEN_RE.finditer(proto_content, brace_index + 1):
        if m.lastgroup == "ignore":
            continue
        if m.group("brace") == "{":
            brace_count += 1
        else:
            brace_count -= 1
            if brace_count == 0:
                return start_index, m.start()

    raise ValueError(f"Could not find closing brace of {block_name}")


def find_oneof_data_bounds(proto_content: str) -> tuple[int, int]:
    """Finds the character index of the start and closing brace for 'oneof data'.

    Args:
        proto_content: The full content of the proto file to search.

    Returns:
        A tuple of (start_index, closing_brace_index) of the 'oneof data' block.

    Raises:
        ValueError: If 'oneof data' or its braces are not found or matched.
    """
    return _find_pattern_bounds(
        proto_content, r"oneof\s+data\b", "'oneof data' block"
    )


def extract_message_def(proto_content: str, message_type: str) -> str:
    """Extracts a message definition along with all its nested contents.

    Args:
        proto_content: The full content of a proto file.
        message_type: The name of the message to extract.

    Returns:
        The extracted message definition string.

    Raises:
        ValueError: If the message or its matching brace is not found.
    """
    start, end = _find_pattern_bounds(
        proto_content,
        rf"message\s+{re.escape(message_type)}\b",
        f"'message {message_type}'",
    )
    return proto_content[start : end + 1]


def _parse_extension_range(item: str) -> tuple[int, int | float] | None:
    """Parses an extension item into a (low, high) tuple or None if invalid."""
    match = _RANGE_RE.match(item.strip())
    if not match:
        return None
    low = int(match.group(1))
    if match.group(2) is None:
        return (low, low)
    high_str = match.group(2)
    high: int | float = float("inf") if high_str == "max" else int(high_str)
    return (low, high) if low <= high else None


def _is_in_range(item: str, target_id: int) -> bool:
    """Checks if a target field ID falls within a single extension item string.

    An extension item can be a single field number (e.g. '76') or a range
    (e.g. '70 to 80' or '1000 to max').

    Args:
        item: The extension item string from a proto extensions declaration.
        target_id: The integer field ID to look for.

    Returns:
        True if target_id is equal to or falls within the item range, False otherwise.
    """
    bounds = _parse_extension_range(item)
    return bounds is not None and bounds[0] <= target_id <= bounds[1]


def _remove_id_from_item(item: str, target_id: int) -> list[str] | None:
    """Removes target_id from an extension item (number or range).

    Args:
        item: The extension item string (e.g. '70 to 80').
        target_id: The integer field ID to remove.

    Returns:
        None if target_id is not in item, or a list of replacement item strings
        (which is empty [] if the entire item is removed).
    """
    bounds = _parse_extension_range(item)
    if bounds is None or not (bounds[0] <= target_id <= bounds[1]):
        return None
    low, high = bounds
    if low == high:
        return []

    res = []
    if low < target_id:
        res.append(
            str(low) if low == target_id - 1 else f"{low} to {target_id - 1}"
        )
    if target_id < high:
        if high == float("inf"):
            res.append(f"{target_id + 1} to max")
        else:
            res.append(
                str(int(high))
                if target_id + 1 == high
                else f"{target_id + 1} to {int(high)}"
            )
    return res


def remove_field_from_extensions(proto_content: str, field_id: str) -> str:
    """Removes a field ID from the extension ranges lists in the proto content.

    Args:
        proto_content: The full content of the proto file.
        field_id: The string field ID to remove.

    Returns:
        The updated proto content with the field ID removed from extension lists.

    Raises:
        ValueError: If no extension range containing the field ID is found.
    """
    if not field_id.isdigit():
        raise ValueError(f"Invalid field ID: {field_id}")
    target_id = int(field_id)

    for match in _EXTENSIONS_TOKEN_RE.finditer(proto_content):
        if match.lastgroup == "ignore" or match.group("stmt") is None:
            continue
        indent = match.group("indent")
        ext_body = match.group("body")
        items = [item.strip() for item in ext_body.split(",")]

        new_items = []
        found = False
        for item in items:
            replacement = _remove_id_from_item(item, target_id)
            if replacement is not None:
                found = True
                new_items.extend(replacement)
            else:
                new_items.append(item)

        if found:
            if not new_items:
                full_start, full_end = match.span(0)
                if (
                    full_end < len(proto_content)
                    and proto_content[full_end] == "\n"
                ):
                    full_end += 1
                return proto_content[:full_start] + proto_content[full_end:]
            else:
                start, end = match.span("stmt")
                if "\n" in ext_body:
                    formatted_items = (",\n" + indent + "  ").join(new_items)
                    new_line = f"extensions\n{indent}  {formatted_items};"
                else:
                    new_line = f"extensions {', '.join(new_items)};"
                return proto_content[:start] + new_line + proto_content[end:]

    raise ValueError(f"Could not find extension range containing {field_id}")


def is_in_extensions(proto_content: str, field_id: str) -> bool:
    """Checks if a field ID is present in any extensions range in the proto.

    Args:
        proto_content: The full content of the proto file.
        field_id: The string field ID to check.

    Returns:
        True if field_id is in an extensions range, False otherwise.
    """
    if not field_id.isdigit():
        return False
    target_id = int(field_id)
    for match in _EXTENSIONS_TOKEN_RE.finditer(proto_content):
        if match.lastgroup == "ignore" or match.group("stmt") is None:
            continue
        ext_body = match.group("body")
        for item in ext_body.split(","):
            if _is_in_range(item, target_id):
                return True
    return False


def merge_extension_field(
    proto_content: str,
    message_type: str,
    field_name: str,
    field_id: str,
) -> str:
    """Inserts the merged field definition into the 'oneof data' block.

    Args:
        proto_content: The content of the main trace proto file.
        message_type: The name of the message type (e.g. FrameTimelineEvent).
        field_name: The name of the field to add in TracePacket (e.g. frame_timeline_event).
        field_id: The field ID to assign to the new field (e.g. 76).

    Returns:
        The updated trace content.

    Raises:
        ValueError: If 'oneof data' block closing brace cannot be found.
    """
    oneof_start, closing_index = find_oneof_data_bounds(proto_content)
    field_indent = None

    block_lines = proto_content[oneof_start:closing_index].splitlines()
    for line in reversed(block_lines):
        line_clean = line.strip()
        if line_clean and not line_clean.startswith(("//", "/*", "*", "oneof")):
            m = re.match(r"^[ \t]*", line)
            if m:
                field_indent = m.group(0)
                break

    if field_indent is None:
        line_start = proto_content.rfind("\n", 0, oneof_start) + 1
        oneof_line = proto_content[line_start:oneof_start]
        m = re.match(r"^[ \t]*", oneof_line)
        oneof_indent = m.group(0) if m else ""
        field_indent = (
            oneof_indent + "  "
            if oneof_indent.isspace() or oneof_indent == ""
            else "    "
        )

    insertion = f"{field_indent}{message_type} {field_name} = {field_id};\n"
    last_newline = proto_content.rfind("\n", 0, closing_index)
    insert_pos = last_newline + 1 if last_newline != -1 else closing_index
    return proto_content[:insert_pos] + insertion + proto_content[insert_pos:]


def find_conflict(oneof_content: str, field_id: str) -> tuple[str, str] | None:
    """Finds if a field ID is defined in the oneof block.

    Args:
        oneof_content: The content of the oneof block.
        field_id: The field ID to check.

    Returns:
        A tuple of (type, field_name) if defined, or None.
    """
    clean_content = _strip_comments(oneof_content)

    # Matches protobuf field declarations for a specific field_id, handling optional field labels
    # and declarations that wrap across multiple lines.
    # Group 1 captures the field type (e.g., 'FrameTimelineEvent' or 'string'), and Group 2 captures
    # the field name (e.g., 'frame_timeline_event').
    # Examples matched:
    #   "    FrameTimelineEvent frame_timeline_event = 76;"
    #   '    optional FrameTimelineEvent frame_timeline_event = 76 [json_name = "frameTimelineEvent"];'
    #   "    FrameTimelineEvent frame_timeline_event =\n        76;"
    pattern = re.compile(
        rf"^[ \t]*(?:\b(?:optional|repeated|required)\s+)?([\w.]+)\s+(\w+)\s*=\s*\b{re.escape(field_id)}\b[^;]*;",
        flags=re.MULTILINE,
    )
    match = pattern.search(clean_content)
    if match:
        return match.group(1), match.group(2)
    return None


def main() -> None:
    """Main entrypoint for resolving proto extensions at build time."""
    parser = argparse.ArgumentParser(
        description="Resolve proto extensions at build time for Perfetto trace proto file"
    )
    parser.add_argument(
        "--input-proto",
        required=True,
        help="Path to input perfetto_trace.proto",
    )
    parser.add_argument(
        "--input-extensions",
        required=True,
        help="Path to input extensions proto",
    )
    parser.add_argument(
        "--output-proto", required=True, help="Path to output proto"
    )
    parser.add_argument(
        "--merge-field-id",
        required=True,
        help="Field ID of the extension to merge back into the main proto",
    )
    parser.add_argument(
        "--merge-message-type",
        required=True,
        help="Name of the message type to merge (e.g. FrameTimelineEvent)",
    )
    parser.add_argument(
        "--merge-field-name",
        required=True,
        help="Name of the field to add in TracePacket (e.g. frame_timeline_event)",
    )

    args = parser.parse_args()

    if not args.merge_field_id.isdigit():
        sys.exit(
            f"Error: Invalid field ID '{args.merge_field_id}'. Must be a positive integer."
        )

    try:
        with open(args.input_proto, "r", encoding="utf-8") as f:
            proto_content = f.read()
    except OSError as e:
        sys.exit(f"Error reading input proto '{args.input_proto}': {e}")

    try:
        oneof_start, closing_index = find_oneof_data_bounds(proto_content)
        oneof_content = proto_content[oneof_start : closing_index + 1]
    except ValueError as e:
        sys.exit(f"Error: {e}")

    conflict = find_conflict(oneof_content, args.merge_field_id)
    if conflict:
        existing_type, existing_field_name = conflict
        if (
            existing_type == args.merge_message_type
            and existing_field_name == args.merge_field_name
        ):
            try:
                with open(args.output_proto, "w", encoding="utf-8") as f:
                    f.write(proto_content)
            except OSError as e:
                sys.exit(
                    f"Error writing output proto '{args.output_proto}': {e}"
                )
            sys.exit(0)
        else:
            sys.exit(
                f"Error: Field ID {args.merge_field_id} is already defined in the trace proto's "
                f"oneof data block with type '{existing_type}' (field name '{existing_field_name}'), "
                f"which conflicts with requested type '{args.merge_message_type}' (field name '{args.merge_field_name}')"
            )

    if not is_in_extensions(proto_content, args.merge_field_id):
        sys.exit(
            f"Error: Field ID {args.merge_field_id} is not being handled as an extension."
        )

    try:
        proto_content = merge_extension_field(
            proto_content,
            args.merge_message_type,
            args.merge_field_name,
            args.merge_field_id,
        )
        proto_content = remove_field_from_extensions(
            proto_content, args.merge_field_id
        )
    except ValueError as e:
        sys.exit(f"Error: {e}")

    try:
        with open(args.input_extensions, "r", encoding="utf-8") as f:
            ext_content = f.read()
    except OSError as e:
        sys.exit(
            f"Error reading extensions proto '{args.input_extensions}': {e}"
        )

    try:
        message_def = extract_message_def(ext_content, args.merge_message_type)
    except ValueError as e:
        sys.exit(f"Error: {e}")

    final_content = (
        f"{proto_content.rstrip()}\n\n"
        f"// Re-added for Fuchsia/Starnix compatibility:\n"
        f"{message_def.strip()}\n"
    )

    try:
        with open(args.output_proto, "w", encoding="utf-8") as f:
            f.write(final_content)
    except OSError as e:
        sys.exit(f"Error writing output proto '{args.output_proto}': {e}")


if __name__ == "__main__":
    main()
