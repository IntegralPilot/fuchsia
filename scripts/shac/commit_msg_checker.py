#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Checks Fuchsia Git commit messages for style guidelines.

Based on //docs/contribute/commit-message-style-guide.md:
- First line summary length (recommended <= 50, warning > 65).
- Body lines wrapped at 72 characters (excluding URLs, footers, and code blocks).
"""

import argparse
import collections
import json
import re
import subprocess
import sys
from typing import List, Literal, Optional, TypedDict

SUBJECT_REC_MAX_LEN = 50
SUBJECT_WARN_MAX_LEN = 65
BODY_MAX_LEN = 72

URL_REGEX = re.compile(r"https?://[^\s]+")
# Footers that are exempt from line length checks
FOOTER_REGEX = re.compile(
    r"^(Bug|Fixed|Test|Change-Id|Cq-Include-Trybots|Fuchsia-Auto-Submit|Multiply|Depends-on|Run-All-Tests|Exempt|No-Tree-Checks|No-Presubmit|No-Try|TAG|CONV)[:=]"
)
# Reverts, relands, and autosquash commits take the original subject and prepend
# a prefix. These are exempt from the length check to avoid manual rewrites of
# auto-generated subjects.
EXEMPT_SUBJECT_PREFIX_REGEX = re.compile(
    r"^(?:Revert(?:\^\d+)?|Reland|fixup!|squash!|amend!) "
)


class MetadataRule(TypedDict):
    mode: Literal["single", "unique_value"]
    level: Literal["warning", "error"]


METADATA_RULES: dict[str, MetadataRule] = {
    "Change-Id": {
        "mode": "single",
        "level": "error",
    },
    "TAG": {
        "mode": "unique_value",
        "level": "warning",
    },
    "CONV": {
        "mode": "unique_value",
        "level": "warning",
    },
}

METADATA_TAG_REGEX = re.compile(
    rf"^({'|'.join(re.escape(k) for k in METADATA_RULES)})[:=]\s*(.+)"
)


class Finding(TypedDict, total=False):
    level: Literal["warning", "error"]
    message: str
    filepath: str
    line: int


class CommitInfo(TypedDict):
    hash: str
    message: str


def check_commit_message(
    message: str, filepath: str = ".git/COMMIT_MSG"
) -> List[Finding]:
    findings: List[Finding] = []
    lines = message.splitlines()
    if not lines:
        return findings

    subject = lines[0]

    # Check subject line length
    if len(
        subject
    ) > SUBJECT_WARN_MAX_LEN and not EXEMPT_SUBJECT_PREFIX_REGEX.match(subject):
        findings.append(
            {
                "level": "warning",
                "message": f"Subject line exceeds {SUBJECT_WARN_MAX_LEN} characters ({len(subject)} chars). Limit first line to {SUBJECT_REC_MAX_LEN}-{SUBJECT_WARN_MAX_LEN} characters.",
                "filepath": filepath,
                "line": 1,
            }
        )

    # Check body line length (wrap at 72 chars) starting from line 3 (if body exists)
    for i, line in enumerate(lines[2:], start=3):
        if len(line) <= BODY_MAX_LEN:
            continue
        # Allow lines containing URLs
        if URL_REGEX.search(line):
            continue
        # Allow standard footers (e.g. Change-Id, Bug, etc.)
        if FOOTER_REGEX.match(line):
            continue
        # Allow indented code block / stack trace lines, quoted revert lines, or comments
        if line.startswith(("    ", "\t", ">", "#")):
            continue
        findings.append(
            {
                "level": "warning",
                "message": f"Line {i} exceeds {BODY_MAX_LEN} characters ({len(line)} chars). Wrap body text at {BODY_MAX_LEN} characters.",
                "filepath": filepath,
                "line": i,
            }
        )

    key_counts: dict[str, int] = collections.defaultdict(int)
    seen_metadata: set[tuple[str, str]] = set()

    for i, line in enumerate(lines, start=1):
        m_meta = METADATA_TAG_REGEX.match(line)
        if not m_meta:
            continue

        key, val = m_meta.group(1), m_meta.group(2).strip()
        rule = METADATA_RULES.get(key)
        if not rule:
            continue

        if rule["mode"] == "single":
            key_counts[key] += 1
            if key_counts[key] > 1:
                findings.append(
                    {
                        "level": rule["level"],
                        "message": f"Multiple '{key}:' footers found. Ensure only one {key} is present.",
                        "filepath": filepath,
                        "line": i,
                    }
                )
        elif rule["mode"] == "unique_value":
            if (key, val) in seen_metadata:
                findings.append(
                    {
                        "level": rule["level"],
                        "message": f"Duplicate '{key}' metadata line found ('{val}'). Ensure duplicate {key} lines are removed.",
                        "filepath": filepath,
                        "line": i,
                    }
                )
            else:
                seen_metadata.add((key, val))
    return findings


def get_git_commit_messages(
    rev: str, cwd: Optional[str] = None
) -> List[CommitInfo]:
    """Returns a list of commit messages for the specified git revision range."""
    cmd = ["git", "log", "--format=%H%x00%B%x00", rev]
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=True,
        )
    except subprocess.CalledProcessError as e:
        sys.stderr.write(f"Failed to read git commit message: {e.stderr}\n")
        return []

    output = proc.stdout
    commits: List[CommitInfo] = []
    parts = output.split("\x00")
    for i in range(0, len(parts) - 1, 2):
        commits.append(CommitInfo(hash=parts[i].strip(), message=parts[i + 1]))
    return commits


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--message", help="Commit message string to check literally."
    )
    parser.add_argument(
        "--message-file", help="Path to file containing commit message."
    )
    parser.add_argument(
        "--rev",
        default="-1",
        help="Git revision or range to check (default: -1 for latest commit).",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output findings formatted as JSON array for Shac.",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit code if any warnings or errors are found.",
    )
    args = parser.parse_args()

    findings: List[Finding] = []

    if args.message is not None:
        findings.extend(check_commit_message(args.message))
    elif args.message_file is not None:
        with open(
            args.message_file, "r", encoding="utf-8", errors="replace"
        ) as f:
            findings.extend(
                check_commit_message(f.read(), filepath=args.message_file)
            )
    else:
        commits = get_git_commit_messages(args.rev)
        for commit in commits:
            findings.extend(check_commit_message(commit["message"]))

    if args.json:
        print(json.dumps(findings, indent=2))
    else:
        for finding in findings:
            level = finding["level"].upper()
            msg = finding["message"]
            line_info = (
                f" (line {finding['line']})" if "line" in finding else ""
            )
            print(f"[{level}]{line_info}: {msg}")

    has_errors = any(f["level"] == "error" for f in findings)
    has_warnings = any(f["level"] == "warning" for f in findings)

    if has_errors or (args.strict and has_warnings):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
