#!/usr/bin/env fuchsia-vendored-python
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import datetime
import glob
import os
import re
import textwrap
from dataclasses import asdict, dataclass
from pathlib import Path
from string import Template

FUCHSIA_DIR = Path(os.environ["FUCHSIA_DIR"])

# Docs paths
INDEX_FILE = FUCHSIA_DIR / "docs/reference/fidl/language/errcat.md"
REDIRECT_FILE = FUCHSIA_DIR / "docs/error/_redirects.yaml"
ERRCAT_DIR = FUCHSIA_DIR / "docs/reference/fidl/language/error-catalog"
ERRCAT_TXT = (
    FUCHSIA_DIR / "docs/reference/fidl/language/error-catalog/_files.txt"
)

# fidlc paths
BAD_FIDL_DIR = FUCHSIA_DIR / "tools/fidl/fidlc/tests/fidl/bad"
BAD_FIDL_TXT = FUCHSIA_DIR / "tools/fidl/fidlc/tests/fidl/bad/files.txt"
GOOD_FIDL_DIR = FUCHSIA_DIR / "tools/fidl/fidlc/tests/fidl/good"
GOOD_FIDL_TXT = FUCHSIA_DIR / "tools/fidl/fidlc/tests/fidl/good/files.txt"
GOOD_TESTS_CC = FUCHSIA_DIR / "tools/fidl/fidlc/tests/errcat_good_tests.cc"

# Templates
DOC_TEMPLATE = (
    FUCHSIA_DIR / "tools/fidl/scripts/add_errcat_templates/doc.md.template"
)
DOC_RETIRED_TEMPLATE = (
    FUCHSIA_DIR
    / "tools/fidl/scripts/add_errcat_templates/doc_retired.md.template"
)
INCLUDECODE_TEMPLATE = (
    FUCHSIA_DIR
    / "tools/fidl/scripts/add_errcat_templates/includecode.md.template"
)
BAD_TEMPLATE = (
    FUCHSIA_DIR / "tools/fidl/scripts/add_errcat_templates/bad.fidl.template"
)
GOOD_TEMPLATE = (
    FUCHSIA_DIR / "tools/fidl/scripts/add_errcat_templates/good.fidl.template"
)

# Regexes used to find insertion locations.
REGEX_INDEX_ENTRY = r"^<<error-catalog/_fi-(\d+)\.md>>"
REGEX_REDIRECT_ENTRY = r"^- from: /docs/error/fi-(\d+)"
REGEX_ERRCAT_TXT_LINE = r"^_fi-(\d+)\.md$"
REGEX_FIDL_TXT_LINE = r"^fi-(\d+)(?:-[a-z])?(?:\.noformat)?\.test.fidl$"
REGEX_GOOD_TEST = r"TEST\(ErrcatGoodTests, Good(\d+)"

# Use this weird string concatenation so that this string literal does not show
# up in search results when people grep for it.
DNS = "DO" + "NOT" + "SUBMIT"

# When true, this script just prints what would happen if run for real.
dry_run = False


def find_error_num(regex, line):
    result = re.search(regex, line)
    if result is None:
        return None
    return int(result.groups(1)[0])


def write_lines(path, lines):
    if not dry_run:
        with open(path, "w") as f:
            f.write("".join(lines))


# Scans each line in path and tries matching with entry_matcher, whose $1 group
# should be 4 digits e.g. 1234 in fi-1234. Once it finds a line where than
# number exceeds num, inserts to_insert just before it. If none exceed num,
# starts at the last matching line, searches for after_regex (if provided), goes
# further down by after_offset lines, and inserts to_insert there.
def insert_entry(
    path, entry_matcher, num, to_insert, after_offset, after_regex=None
):
    with open(path) as f:
        lines = f.readlines()

    # Do a pass over the lines in the file to ensure that this entry does not already exist.
    if any(line.startswith(to_insert.split("\n")[0]) for line in lines):
        return 0

    # If we do need a file do another pass, inserting the supplied text in the correct position.
    insert_after = None
    for line_num, line in enumerate(lines):
        entry_num = find_error_num(entry_matcher, line)
        if entry_num is None:
            continue
        if entry_num > num:
            lines.insert(line_num, to_insert + "\n")
            write_lines(path, lines)
            return 1
        insert_after = line_num

    # Handle the edge case of the entry needing to be placed at the end of the list.
    if insert_after is None:
        raise Exception(f"never found regex {entry_matcher} in {path}")
    if after_regex:
        while insert_after < len(lines) and not re.search(
            after_regex, lines[insert_after]
        ):
            insert_after += 1
    lines.insert(insert_after + after_offset, to_insert + "\n")
    write_lines(path, lines)
    return 1


# The opposite of insert_entry. Removes the lines where entry_matcher matches
# and its $1 group is num. Removes additional lines if after_regex is given.
def remove_entry(path, entry_matcher, num, after_regex=None):
    with open(path) as f:
        lines = f.readlines()
    i = 0
    new_lines = []
    delta = 0
    while i < len(lines):
        line = lines[i]
        entry_num = find_error_num(entry_matcher, line)
        if entry_num == num:
            delta -= 1
            if after_regex:
                while i < len(lines):
                    if re.search(after_regex, lines[i]):
                        break
                    i += 1
        else:
            new_lines.append(line)
        i += 1
    write_lines(path, new_lines)
    return delta


def substitute(template_path, subs):
    subs["year"] = datetime.date.today().year
    subs["dns"] = DNS
    with open(template_path) as f:
        template = Template(f.read())
    return template.substitute(subs)


def create_file(file_path, template_path, subs, overwrite_existing):
    old = ""
    if file_path.is_file():
        if not overwrite_existing:
            return 0
        with open(file_path) as f:
            old = f.read()
    new = substitute(template_path, subs)
    if not dry_run:
        with open(file_path, "w") as f:
            f.write(new)
    if old == new:
        return 0
    return 1


def remove_file(file_path):
    if not file_path.is_file():
        return 0
    if not dry_run:
        os.remove(file_path)
    return -1


def markdown_filename(n):
    return f"_fi-{n:04}.md"


@dataclass
class Change:
    markdown_file: int = 0
    markdown_txt_line: int = 0
    index_entry: int = 0
    redirect_entry: int = 0
    bad_fidl_files: int = 0
    good_fidl_files: int = 0
    bad_txt_lines: int = 0
    good_txt_lines: int = 0
    good_tests: int = 0


def perform_change(args) -> Change:
    change = Change()
    n = args.numeral
    ns = f"{n:04}"

    # Add an index entry if none exists.
    change.index_entry = insert_entry(
        INDEX_FILE,
        REGEX_INDEX_ENTRY,
        n,
        f"<<error-catalog/_fi-{ns}.md>>\n",
        after_offset=2,
    )

    # Add a redirect entry if none exists.
    change.redirect_entry = insert_entry(
        REDIRECT_FILE,
        REGEX_REDIRECT_ENTRY,
        n,
        f"- from: /docs/error/fi-{ns}\n  to: /docs/reference/fidl/language/errcat.md#fi-{ns}",
        after_offset=2,
    )

    # Create/remove .test.fidl files for "bad" examples, and update files.txt.
    bad_includecodes = []
    if args.retire:
        for path_str in glob.glob(f"{BAD_FIDL_DIR}/fi-{ns}*.test.fidl"):
            change.bad_fidl_files += remove_file(Path(path_str))
        change.bad_txt_lines += remove_entry(
            BAD_FIDL_TXT, REGEX_FIDL_TXT_LINE, n
        )
    else:
        for b in range(args.bad):
            suffix = chr(ord("a") + b) if args.bad > 1 else ""
            case_name = "fi-" + ns + ("-" + suffix if suffix else "")
            flat_name = ns + suffix

            # Add any new files we need.
            filename = f"{case_name}.test.fidl"
            change.bad_fidl_files += create_file(
                file_path=BAD_FIDL_DIR / filename,
                template_path=BAD_TEMPLATE,
                subs={
                    "num": ns,
                    "case_name": case_name,
                    "flat_name": flat_name,
                },
                overwrite_existing=False,
            )

            # Insert this filename into files.txt.
            change.bad_txt_lines += insert_entry(
                BAD_FIDL_TXT, REGEX_FIDL_TXT_LINE, n, filename, after_offset=1
            )

            # Create the markdown entry for this case.
            bad_includecodes.append(
                substitute(
                    INCLUDECODE_TEMPLATE,
                    {
                        "kind": "bad",
                        "case_name": case_name,
                    },
                )
            )

    # Create .test.fidl files for "good" examples, and add to BUILD.gn.
    good_includecodes = []
    if args.retire:
        for path_str in glob.glob(f"{GOOD_FIDL_DIR}/fi-{ns}*.test.fidl"):
            change.good_fidl_files += remove_file(Path(path_str))
        change.good_txt_lines += remove_entry(
            GOOD_FIDL_TXT, REGEX_FIDL_TXT_LINE, n
        )
        change.good_tests += remove_entry(
            GOOD_TESTS_CC,
            REGEX_GOOD_TEST,
            n,
            after_regex=r"^}$",
        )
    else:
        for g in range(args.good):
            suffix = chr(ord("a") + g) if args.good > 1 else ""
            case_name = "fi-" + ns + ("-" + suffix if suffix else "")
            flat_name = ns + suffix

            # Add any new files we need.
            filename = f"{case_name}.test.fidl"
            change.good_fidl_files += create_file(
                file_path=GOOD_FIDL_DIR / filename,
                template_path=GOOD_TEMPLATE,
                subs={
                    "num": ns,
                    "case_name": case_name,
                    "flat_name": flat_name,
                },
                overwrite_existing=False,
            )

            # Insert this filename into files.txt.
            change.good_txt_lines += insert_entry(
                GOOD_FIDL_TXT, REGEX_FIDL_TXT_LINE, n, filename, after_offset=1
            )

            # Create the markdown entry for this case.
            good_includecodes.append(
                substitute(
                    INCLUDECODE_TEMPLATE,
                    {
                        "kind": "good",
                        "case_name": case_name,
                    },
                )
            )

            # Add a test case if none exists.
            insert = textwrap.dedent(
                f"""\
                TEST(ErrcatGoodTests, Good{flat_name}) {{
                TestLibrary library;
                library.AddFile("good/{filename}");
                ASSERT_COMPILED(library);
                }}
                """
            )
            change.good_tests += insert_entry(
                GOOD_TESTS_CC,
                REGEX_GOOD_TEST,
                n,
                insert,
                after_offset=2,
                after_regex=r"^}$",
            )

    # Create the Markdown file for the actual doc.
    md_filename = markdown_filename(n)
    change.markdown_file = create_file(
        file_path=ERRCAT_DIR / md_filename,
        template_path=DOC_RETIRED_TEMPLATE if args.retire else DOC_TEMPLATE,
        subs={
            "num": ns,
            "good_includecodes": "\n".join(good_includecodes),
            "bad_includecodes": "\n".join(bad_includecodes),
        },
        overwrite_existing=args.retire,
    )

    # Insert the Markdown filename into _files.txt.
    change.markdown_txt_line += insert_entry(
        ERRCAT_TXT, REGEX_ERRCAT_TXT_LINE, n, md_filename, after_offset=1
    )

    return change


def main(args):
    global dry_run
    dry_run = args.dry_run
    change = perform_change(args)
    if not (any(asdict(change).values())):
        print(f"There is nothing to do for fi-{args.numeral:04}")
        return

    def report(count, kind, path):
        if count == 0:
            return
        if count > 0:
            action = "Would add" if dry_run else "Added"
        else:
            action = "Would remove" if dry_run else "Removed"
        print(f"* {action} {abs(count)} {kind}: {path.relative_to(Path.cwd())}")

    report(
        change.markdown_file,
        "file",
        ERRCAT_DIR / markdown_filename(args.numeral),
    )
    report(change.markdown_txt_line, "line", ERRCAT_TXT)
    report(change.index_entry, "index entry", INDEX_FILE)
    report(change.redirect_entry, "redirect entry", REDIRECT_FILE)
    report(change.bad_fidl_files, "file(s)", BAD_FIDL_DIR)
    report(change.good_fidl_files, "file(s)", GOOD_FIDL_DIR)
    report(change.bad_txt_lines, "line(s)", BAD_FIDL_TXT)
    report(change.good_txt_lines, "line(s)", GOOD_FIDL_TXT)
    report(change.good_tests, "test case(s)", GOOD_TESTS_CC)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description=textwrap.dedent(
            """
            Adds an entry to //docs/references/fidl/language/errcat.md,
            generating or updating all files as necessary to document the error.
            It is safe to run multiple times. It only adds things that are missing.
            """
        ),
        formatter_class=argparse.RawTextHelpFormatter,
    )
    parser.add_argument(
        "numeral",
        metavar="NUMBER",
        type=int,
        help="The error ID number as defined in diagnostics.h",
    )
    parser.add_argument(
        "-b",
        "--bad",
        type=int,
        default=1,
        help='Number of "bad" example .test.fidl files to generate',
    )
    parser.add_argument(
        "-g",
        "--good",
        type=int,
        default=1,
        help='Number of "good" example .test.fidl files to generate',
    )
    parser.add_argument(
        "-r",
        "--retire",
        action="store_true",
        help="Retire the given error number",
    )
    parser.add_argument(
        "-n",
        "--dry-run",
        action="store_true",
        help="Just print what would happen, do not create or edit files",
    )
    main(parser.parse_args())
