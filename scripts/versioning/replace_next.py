#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""
Attempts to replace all instances of the `NEXT` pseudo-API-level with a
concrete value.

This is done heuristically, based on text. Always do a subsequent codebase-wide
regular expression search for `\bNEXT\b` after running this script, and ensure
it got everything. If you find any coverage gaps, please amend this script to
fix them.

Usage:

    ./scripts/versioning/replace_next.py --fuchsia-dir $PWD --new-api-level 24
"""
import argparse
import concurrent.futures
import dataclasses
import pathlib
import re
import subprocess
from typing import Generator

IGNORED_RELATIVE_PATHS: set[pathlib.Path] = {
    pathlib.Path("tools/fidl/fidlc/testdata/versions.test.fidl"),
    pathlib.Path("zircon/system/availability_test.c"),
    pathlib.Path("tools/fidl/fidlc/tests/versioning_attribute_tests.cc"),
}


@dataclasses.dataclass
class Pattern:
    # `sub_regex` will only be applied to files whose path (relative to
    # FUCHSIA_DIR) matches this regex.
    path_regex: re.Pattern[str]

    # Substitution to apply to the file. Must have a capture group named
    # "prefix" and another named "suffix". Any matches will be replaced with
    # <prefix>new_api_level<suffix>.
    sub_regex: re.Pattern[str]

    def path_matches(self, rel_path: pathlib.Path) -> bool:
        """Returns true if this pattern should apply to the given file."""
        return bool(self.path_regex.search(str(rel_path)))

    def substitute(self, text: str, new_api_level: str) -> str:
        """Performs the substitution and returns the result."""

        def sub_fn(m: re.Match[str]) -> str:
            position_of_next = m.start() + len(m.group("prefix"))
            start_of_line = text.rfind("\n", 0, position_of_next) + 1
            end_of_line = text.find("\n", position_of_next)
            if end_of_line == -1:
                end_of_line = len(text)

            line = text[start_of_line:end_of_line]

            if "NEVER_REPLACE_NEXT" in line:
                return m.group(0)

            return m.group("prefix") + new_api_level + m.group("suffix")

        return self.sub_regex.sub(sub_fn, text)


# `json` is included because magma.json is used to generate a header and thus can
# contain the macros.
CPP_PATH_REGEX: re.Pattern[str] = re.compile(r"\.(h|c|cc|json)$")


PATTERNS = [
    Pattern(
        path_regex=CPP_PATH_REGEX,
        # Matches macros like `FUCHSIA_API_LEVEL_AT_LEAST(NEXT)`.
        #
        # Possible coverage gaps:
        # - FUCHSIA_API_LEVEL_AT_LEAST( NEXT )
        sub_regex=re.compile(r"(?P<prefix>\()NEXT(?P<suffix>\))"),
    ),
    Pattern(
        path_regex=CPP_PATH_REGEX,
        # Matches annotations like `ZX_AVAILABLE_SINCE(NEXT)` or
        # `ZX_REMOVED_SINCE(1, 19,
        #                   NEXT,
        #                   "Use DoNewThing() instead")`
        #
        # Possible coverage gaps:
        # - ZX_REMOVED_SINCE((1), 19, NEXT)
        sub_regex=re.compile(
            r"(?P<prefix>_SINCE\([^)]*)NEXT(?P<suffix>)", re.MULTILINE
        ),
    ),
    Pattern(
        path_regex=re.compile(r"\.rs$"),
        # Matches `cfg` predicates like `fuchsia_api_level_at_least = "NEXT"`.
        # Possible coverage gaps:
        # - Multiple instances on the same line?
        sub_regex=re.compile(
            r"(?P<prefix>fuchsia_api_level_.*\")NEXT(?P<suffix>\")",
            re.MULTILINE,
        ),
    ),
    Pattern(
        path_regex=re.compile(r"\.fidl$"),
        # Matches annotations like `@available(added=NEXT)` or
        # `@available(added=15, deprecated=22, removed=NEXT)`.
        #
        # Possible coverage gaps:
        # - The exclusion of `[^_;(A-Z]` is somewhat arbitrary.
        sub_regex=re.compile(
            r"(?P<prefix>= *)NEXT(?P<suffix>[^_;(A-Z])", re.MULTILINE
        ),
    ),
]


def get_git_files_for_substitution(
    fuchsia_dir: pathlib.Path,
) -> Generator[pathlib.Path, None, None]:
    """Calls `git ls-files` in the given directory and filters out files that
    cannot have relevant matches."""
    git_ls_files_out = subprocess.check_output(
        ["git", "-C", str(fuchsia_dir), "ls-files"], text=True
    )

    for line in git_ls_files_out.splitlines():
        path = pathlib.Path(line)
        if all(
            [
                path not in IGNORED_RELATIVE_PATHS,
                not path.is_dir(),
                any(p.path_matches(path) for p in PATTERNS),
            ]
        ):
            yield path


def do_subsitituion(path: pathlib.Path, new_api_level: str) -> None:
    """Replaces `NEXT` with new_api_level in the given file."""
    try:
        with path.open() as f:
            contents = f.read()
    except UnicodeDecodeError:
        print("Encountered binary file: " + str(path))
        return

    new_contents = contents
    for p in PATTERNS:
        if p.path_matches(path):
            new_contents = p.substitute(new_contents, new_api_level)

    if new_contents != contents:
        with path.open("w") as f:
            f.write(new_contents)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fuchsia-dir", type=pathlib.Path, required=True)
    parser.add_argument("--new-api-level", type=str, required=True)

    args = parser.parse_args()

    with concurrent.futures.ThreadPoolExecutor() as exe:
        # Use `list` to drain the iterator.
        list(
            exe.map(
                lambda path: do_subsitituion(path, args.new_api_level),
                get_git_files_for_substitution(args.fuchsia_dir),
            )
        )


if __name__ == "__main__":
    main()
