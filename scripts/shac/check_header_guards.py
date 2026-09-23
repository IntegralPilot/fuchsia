#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Script to check C and C++ file header guards.

This script accepts a list of file or directory arguments. If a given
path is a file, it runs the checker on it. If the path is a directory,
it runs the checker on all .h files in that directory.

Supports:
  --fix: Updates header guards in-place using atomic file writes.
  --emit: Prints the corrected file content to stdout without writing to disk
          (used by SHAC formatters running in read-only sandboxes).
  --root: Sets the repository root against which relative paths are resolved.
"""

import argparse
import collections
import os
import pathlib
import re
import shutil
import sys
import tempfile
from collections.abc import Collection

# Prefixes that are stripped or adjusted to form canonical header guards.
_SYSROOT_PREFIXES = frozenset({"ZIRCON_THIRD_PARTY_ULIB_MUSL_INCLUDE"})
_SYSROOT_PREFIX_PATTERN = re.compile(
    "^(" + "|".join(sorted(_SYSROOT_PREFIXES)) + ")_"
)

_PUBLIC_PREFIXES = frozenset(
    {
        "ZIRCON_SYSTEM_PUBLIC",
        "ZIRCON_SYSTEM_ULIB_.*_INCLUDE",
        "SDK(_.*_INCLUDE)?",
    }
)
_PUBLIC_PREFIX_PATTERN = re.compile(
    "^(" + "|".join(sorted(_PUBLIC_PREFIXES)) + ")_"
)

_PRAGMA_ONCE_PATTERN = re.compile(r"^\s*#\s*pragma\s+once\b")
_DISALLOWED_HEADER_CHARACTERS = re.compile(r"[^A-Z0-9_]")
_IFNDEF_PATTERN = re.compile(r"^\s*#\s*ifndef\s+(\S+)")
_DEFINE_PATTERN = re.compile(r"^\s*#\s*define\s+(\S+)")
_ENDIF_PATTERN = re.compile(r"^\s*#\s*endif\b")


class HeaderGuardError(Exception):
    """Base exception for header guard errors."""


class HeaderGuardFixError(HeaderGuardError, ValueError):
    """Raised when file content cannot be safely fixed."""


def _adjust_for_location(header_guard: str) -> str:
    """Remove internal location prefixes from public and sysroot headers."""
    return _SYSROOT_PREFIX_PATTERN.sub(
        "SYSROOT_",
        _PUBLIC_PREFIX_PATTERN.sub("", header_guard, 1),
        1,
    )


def _header_guard_from_path(path: pathlib.Path, root: pathlib.Path) -> str:
    """Compute the canonical header guard from the file path relative to root."""
    try:
        relative_path = path.resolve().relative_to(root.resolve())
    except ValueError as e:
        # Avoid crashing on path resolution mismatches; provide clear diagnostic.
        raise HeaderGuardError(
            f"Path '{path}' is not within root '{root}'"
        ) from e

    upper_path = str(relative_path).upper()
    return _adjust_for_location(
        f"{_DISALLOWED_HEADER_CHARACTERS.sub('_', upper_path)}_"
    )


def _generate_fixed_content(content: str, expected_guard: str) -> str:
    """Generates the corrected file content with standard header guards.

    Unlike blanket line-by-line regex replacements, this targets only the outer
    guard directives (first #ifndef/#define and final closing #endif). This avoids
    corrupting internal preprocessor checks such as '#ifndef __cplusplus' or
    feature macros.

    Raises:
        HeaderGuardFixError: If the header is malformed and cannot be safely fixed.
    """
    lines = content.splitlines(keepends=True)
    if not lines:
        return f"#ifndef {expected_guard}\n#define {expected_guard}\n\n#endif  // {expected_guard}\n"

    pragma_indices = tuple(
        index
        for index, line in enumerate(lines)
        if _PRAGMA_ONCE_PATTERN.match(line) is not None
    )
    if len(pragma_indices) > 1:
        # Multiple #pragma once indicates a malformed header that should not be auto-fixed.
        raise HeaderGuardFixError(
            "Multiple #pragma once directives indicate a malformed header that should not be auto-fixed."
        )

    # Convert #pragma once into standard Google/Fuchsia include guards.
    if pragma_indices:
        pragma_index = pragma_indices[0]
        lines[
            pragma_index
        ] = f"#ifndef {expected_guard}\n#define {expected_guard}\n"
        if not lines[-1].endswith("\n"):
            lines[-1] += "\n"
        lines.append(f"\n#endif  // {expected_guard}\n")
        return "".join(lines)

    # Find the outer guard: first #ifndef and last #endif
    first_ifndef_index = None
    first_ifndef_macro = None
    for index, line in enumerate(lines):
        if (ifndef_match := _IFNDEF_PATTERN.match(line)) is not None:
            first_ifndef_index = index
            first_ifndef_macro = ifndef_match.group(1)
            break

    last_endif_index = None
    for index in range(len(lines) - 1, -1, -1):
        if _ENDIF_PATTERN.match(lines[index]) is not None:
            last_endif_index = index
            break

    if first_ifndef_index is not None and last_endif_index is not None:
        if first_ifndef_index >= last_endif_index:
            raise HeaderGuardFixError(
                f"Malformed header guard: #ifndef at line {first_ifndef_index + 1} "
                f"appears after #endif at line {last_endif_index + 1}."
            )
        # Find matching or immediately following #define
        define_index = None
        for index in range(first_ifndef_index + 1, last_endif_index):
            if (
                define_match := _DEFINE_PATTERN.match(lines[index])
            ) is not None:
                if define_match.group(1) == first_ifndef_macro:
                    define_index = index
                    break
                if define_index is None:
                    define_index = index

        lines[first_ifndef_index] = f"#ifndef {expected_guard}\n"
        if define_index is not None:
            lines[define_index] = f"#define {expected_guard}\n"
        else:
            lines.insert(first_ifndef_index + 1, f"#define {expected_guard}\n")
            last_endif_index += 1
        lines[last_endif_index] = f"#endif  // {expected_guard}\n"
        return "".join(lines)

    if first_ifndef_index is not None:
        raise HeaderGuardFixError(
            f"Malformed header guard: unmatched #ifndef at line {first_ifndef_index + 1}."
        )

    if last_endif_index is not None:
        raise HeaderGuardFixError(
            f"Malformed header guard: unmatched #endif at line {last_endif_index + 1}."
        )

    # If no existing header guard directives were found, insert at the top
    # after any leading license / block comments.
    insert_index = 0
    in_block_comment = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if in_block_comment:
            if "*/" in stripped:
                in_block_comment = False
            insert_index = index + 1
        elif stripped.startswith("/*"):
            if "*/" not in stripped:
                in_block_comment = True
            insert_index = index + 1
        elif stripped.startswith("//") or not stripped:
            insert_index = index + 1
        else:
            break

    guard_start = f"#ifndef {expected_guard}\n#define {expected_guard}\n\n"
    lines.insert(insert_index, guard_start)
    if not lines[-1].endswith("\n"):
        lines[-1] += "\n"
    lines.append(f"\n#endif  // {expected_guard}\n")
    return "".join(lines)


def _write_atomically(path: pathlib.Path, content: str) -> None:
    """Writes content to a tempfile in the target directory and renames atomically.

    This prevents file corruption if the process crashes or is interrupted mid-write.
    """
    temp_file_path = None
    try:
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=path.parent,
            delete=False,
        ) as temp_file:
            temp_file_path = pathlib.Path(temp_file.name)
            temp_file.write(content)
        shutil.copymode(path, temp_file_path)
        temp_file_path.replace(path)
    finally:
        if temp_file_path is not None and temp_file_path.exists():
            temp_file_path.unlink()


def _check_file(
    path: pathlib.Path,
    root: pathlib.Path,
    fix_guards: bool,
    emit: bool,
    collision_tracker: dict[str, list[pathlib.Path]],
) -> None:
    """Check or format the header guard for a given header file."""
    if path.suffix != ".h":
        return

    expected_guard = _header_guard_from_path(path, root)
    collision_tracker[expected_guard].append(path)

    try:
        content = path.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        raise HeaderGuardError(f"Error reading {path}: {e}") from e

    lines = content.splitlines()

    # Directives matching the expected guard
    ifndef_re = re.compile(rf"^#ifndef\s+{re.escape(expected_guard)}$")
    define_re = re.compile(rf"^#define\s+{re.escape(expected_guard)}$")
    endif_re = re.compile(
        rf"^#endif\s+(?://\s*|/\*\s*){re.escape(expected_guard)}(?:\s*\*/)?\s*$"
    )

    found_pragma_once = False
    found_ifndef = False
    found_define = False
    found_endif = False

    for line in lines:
        if _PRAGMA_ONCE_PATTERN.match(line) is not None:
            if found_pragma_once:
                raise HeaderGuardError(f"{path} contains multiple #pragma once")
            found_pragma_once = True

        if ifndef_re.match(line) is not None:
            if found_ifndef:
                raise HeaderGuardError(
                    f"{path} contains multiple ifndef header guards"
                )
            found_ifndef = True

        if define_re.match(line) is not None:
            if found_define:
                raise HeaderGuardError(
                    f"{path} contains multiple define header guards"
                )
            found_define = True

        if endif_re.match(line) is not None:
            if found_endif:
                raise HeaderGuardError(
                    f"{path} contains multiple endif header guards"
                )
            found_endif = True

    if found_pragma_once:
        if found_ifndef or found_define or found_endif:
            raise HeaderGuardError(
                f"{path} contains both #pragma once and header guards"
            )
        # If neither fixing nor emitting, #pragma once is accepted as valid.
        if not fix_guards and not emit:
            return

    # Standard include guard match
    is_valid = found_ifndef and found_define and found_endif

    if is_valid:
        if emit:
            sys.stdout.write(content)
        return

    # Guard needs fix or emission of corrected content.
    # In check mode, report the specific missing component.
    if not fix_guards and not emit:
        if not found_ifndef:
            raise HeaderGuardError(
                f"{path} missing #ifndef part of header guard (expected: {expected_guard})"
            )
        if not found_define:
            raise HeaderGuardError(
                f"{path} missing #define part of header guard (expected: {expected_guard})"
            )
        raise HeaderGuardError(
            f"{path} missing matching #endif comment (expected: #endif  // {expected_guard})"
        )

    # In fix or emit mode, generate the fixed content.
    fixed_content = _generate_fixed_content(content, expected_guard)

    if emit:
        sys.stdout.write(fixed_content)
        return

    assert fix_guards
    try:
        _write_atomically(path, fixed_content)
        print(f"Fixed header guard in {path}")
    except OSError as e:
        raise HeaderGuardError(f"Error writing {path}: {e}") from e


def _find_header_files(
    dir_path: pathlib.Path,
) -> Collection[pathlib.Path]:
    """Finds all .h files in dir_path, pruning hidden and third_party directories."""
    headers = []
    for current_root, dirs, files in os.walk(dir_path):
        # Prune dot directories (e.g. .git) and third_party directories
        dirs[:] = [
            d for d in dirs if not d.startswith(".") and d != "third_party"
        ]
        for file_name in files:
            if file_name.endswith(".h"):
                headers.append(pathlib.Path(current_root) / file_name)
    return tuple(headers)


def _check_collisions(
    collision_tracker: dict[str, list[pathlib.Path]],
) -> None:
    """Checks whether multiple files share the same computed header guard."""
    collision_messages = []
    for header_guard, paths in collision_tracker.items():
        if len(paths) > 1:
            formatted_paths = "\n".join(f"    {path}" for path in paths)
            collision_messages.append(
                f"Multiple files could use {header_guard} as a header guard:\n{formatted_paths}"
            )
    if collision_messages:
        raise HeaderGuardError("\n".join(collision_messages))


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check and fix C/C++ header guards."
    )
    parser.add_argument(
        "--fix",
        help="Correct wrong header guards in-place atomically.",
        action="store_true",
    )
    parser.add_argument(
        "--emit",
        help="Emit corrected file content to stdout without modifying disk.",
        action="store_true",
    )
    parser.add_argument(
        "--root",
        help="Root of the repository or checkout to calculate relative guards against.",
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help="Files or directories to check/fix.",
    )

    args = parser.parse_args()

    default_root = pathlib.Path(__file__).resolve().parents[2]
    root = (
        pathlib.Path(args.root).resolve()
        if args.root
        else pathlib.Path(os.environ.get("FUCHSIA_DIR", default_root)).resolve()
    )

    collision_tracker: dict[str, list[pathlib.Path]] = collections.defaultdict(
        list
    )
    all_ok = True
    paths_to_check: list[pathlib.Path] = []

    for path_arg in args.paths:
        resolved_path = pathlib.Path(path_arg).resolve()
        if resolved_path.is_dir():
            paths_to_check.extend(_find_header_files(resolved_path))
        elif resolved_path.is_file():
            paths_to_check.append(resolved_path)
        else:
            print(f"Path does not exist: {resolved_path}", file=sys.stderr)
            all_ok = False

    for path in paths_to_check:
        try:
            _check_file(
                path,
                root,
                fix_guards=args.fix,
                emit=args.emit,
                collision_tracker=collision_tracker,
            )
        except HeaderGuardError as e:
            print(e, file=sys.stderr)
            all_ok = False

    try:
        _check_collisions(collision_tracker)
    except HeaderGuardError as e:
        print(e, file=sys.stderr)
        all_ok = False

    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
