# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Structured summary reporting for `fx test`.

Provides models and utilities for generating a unified machine-readable
summary JSON of test execution (including both host and device tests),
as well as tracking per-test and build log artifact paths.
"""

from __future__ import annotations

from dataclasses import dataclass
from dataclasses import field
import json
import os
import re
import typing


@dataclass
class Suggestion:
    """Represents a structured suggestion when a test target was not found."""

    name: str
    similarity: float | None = None
    build_includes: str | None = None
    add_test_command: str | None = None

    def to_dict(self) -> dict[str, typing.Any]:
        d: dict[str, typing.Any] = {
            "name": str(self.name),
        }
        if self.similarity is not None:
            d["similarity"] = float(self.similarity)
        if self.build_includes is not None:
            d["build_includes"] = str(self.build_includes)
        if self.add_test_command is not None:
            d["add_test_command"] = str(self.add_test_command)
        return d


def parse_suggestions_from_output(raw_output: str) -> list[Suggestion]:
    """Parse raw output lines from search-tests into structured Suggestion objects."""
    suggestions: list[Suggestion] = []
    current_suggestion: Suggestion | None = None

    for line in raw_output.splitlines():
        line_str = line.strip()
        if (
            not line_str
            or line_str.startswith("(")
            and line_str.endswith("matches not shown)")
        ):
            continue

        similarity_match = re.match(
            r"^(.+?)\s*\(([0-9.]+)%\s*similar\)", line_str
        )
        if similarity_match:
            if current_suggestion is not None:
                suggestions.append(current_suggestion)
            name = similarity_match.group(1).strip()
            similarity = float(similarity_match.group(2)) / 100.0
            current_suggestion = Suggestion(name=name, similarity=similarity)
        elif current_suggestion is not None:
            if line_str.startswith("Build includes:"):
                current_suggestion.build_includes = line_str.removeprefix(
                    "Build includes:"
                ).strip()
            elif line_str.startswith(("fx add-test", "fx add-host-test")):
                current_suggestion.add_test_command = line_str
            else:
                suggestions.append(current_suggestion)
                current_suggestion = Suggestion(name=line_str)
        else:
            if line_str.startswith(
                ("Build includes:", "fx add-test", "fx add-host-test")
            ):
                continue
            current_suggestion = Suggestion(name=line_str)

    if current_suggestion is not None:
        suggestions.append(current_suggestion)

    return suggestions


@dataclass
class BuildResult:
    """Represents the outcome of the build phase before test execution."""

    status: str
    exit_code: int | None = None
    log_path: str | None = None
    error: str | None = None

    def to_dict(self) -> dict[str, typing.Any]:
        d: dict[str, typing.Any] = {
            "status": str(self.status),
        }
        if self.exit_code is not None:
            d["exit_code"] = int(self.exit_code)
        if self.log_path is not None and isinstance(self.log_path, str):
            d["log_path"] = self.log_path
        if self.error is not None and isinstance(self.error, str):
            d["error"] = self.error
        return d


@dataclass
class TestResult:
    """Represents the execution outcome of an individual test suite."""

    name: str
    type: str  # "host" or "device"
    outcome: str  # "PASSED", "FAILED", "SKIPPED", "TIMEOUT", "ABORTED", "FAILED_TO_START"
    exit_code: int | None = None
    log_path: str | None = None
    stdout_log_path: str | None = None
    stderr_log_path: str | None = None
    duration_seconds: float | None = None
    message: str | None = None

    def to_dict(self) -> dict[str, typing.Any]:
        d: dict[str, typing.Any] = {
            "name": str(self.name),
            "type": str(self.type),
            "outcome": str(self.outcome),
        }
        if self.exit_code is not None:
            d["exit_code"] = int(self.exit_code)
        if self.log_path is not None and isinstance(self.log_path, str):
            d["log_path"] = self.log_path
        if self.stdout_log_path is not None and isinstance(
            self.stdout_log_path, str
        ):
            d["stdout_log_path"] = self.stdout_log_path
        if self.stderr_log_path is not None and isinstance(
            self.stderr_log_path, str
        ):
            d["stderr_log_path"] = self.stderr_log_path
        if self.duration_seconds is not None:
            if isinstance(self.duration_seconds, (int, float)):
                d["duration_seconds"] = float(self.duration_seconds)
            else:
                try:
                    d["duration_seconds"] = float(str(self.duration_seconds))
                except (ValueError, TypeError):
                    pass
        if self.message is not None and isinstance(self.message, str):
            d["message"] = self.message
        return d


@dataclass
class RunSummary:
    """Unified summary of an fx test run."""

    outcome: str = "PASSED"  # "PASSED", "FAILED", "BUILD_FAILED", "ERROR"
    summary_path: str | None = None
    error: str | None = None
    total_passed: int = 0
    total_failed: int = 0
    total_skipped: int = 0
    build: BuildResult | None = None
    tests: list[TestResult] = field(default_factory=list)
    suggestions: list[Suggestion] = field(default_factory=list)
    hints: list[str] = field(default_factory=list)

    def add_test(self, test: TestResult) -> None:
        """Add a test result and update counters."""
        self.tests.append(test)
        if test.outcome == "PASSED":
            self.total_passed += 1
        elif test.outcome == "SKIPPED":
            self.total_skipped += 1
        else:
            self.total_failed += 1

    def to_markdown(self) -> str:
        """Returns a Markdown string representation of the test suite."""

        # Determine overall outcome, factoring in build failures
        if self.build and getattr(self.build, "status", "PASSED") != "PASSED":
            self.outcome = "BUILD_FAILED"

        lines = [f"# Test Run: {self.outcome}"]

        # Build Status
        if self.build:
            lines.append(f"- **Build**: {self.build.status}")
            if getattr(self.build, "exit_code", None) is not None:
                lines.append(f"  - exit code: {self.build.exit_code}")
            if getattr(self.build, "log_path", None) is not None:
                lines.append(f"  - logs: `{self.build.log_path}`")
            if getattr(self.build, "error", None) is not None:
                lines.append(f"  - error: {self.build.error}")

        # Add basic stats
        lines.append(
            f"- **Results**: {self.total_passed} Passed | {self.total_failed} Failed | {self.total_skipped} Skipped"
        )

        if self.summary_path:
            lines.append(f"- **Summary JSON**: `{self.summary_path}`")

        lines.append("")

        # Test case details
        if self.tests:
            lines.append("## Tests")
            for t in self.tests:
                duration_str = (
                    f"{t.duration_seconds:.2f}s"
                    if t.duration_seconds is not None
                    else "0.00s"
                )
                lines.append(f"- [{t.outcome}] `{t.name}` ({duration_str})")

                if t.stdout_log_path:
                    lines.append(f"  - stdout: `{t.stdout_log_path}`")
                elif t.log_path:
                    # Fallback to general log_path if stdout is not set
                    lines.append(f"  - stdout: `{t.log_path}`")

                if t.stderr_log_path:
                    lines.append(f"  - stderr: `{t.stderr_log_path}`")
                if t.message:
                    lines.append(f"  - Message: {t.message}")

        if self.suggestions:
            lines.append("")
            lines.append("## Suggestions")
            for s in self.suggestions:
                if s.similarity is not None:
                    sim_pct = int(s.similarity * 100)
                    lines.append(f"- **{s.name}** ({sim_pct}% match)")
                else:
                    lines.append(f"- **{s.name}**")
                if s.add_test_command:
                    lines.append(f"  - Run: `{s.add_test_command}`")

        if self.hints:
            lines.append("")
            lines.append("## Hints")
            for h in self.hints:
                lines.append(f"- {h}")

        return "\n".join(lines)

    def finalize(self, error: str | None = None) -> None:
        """Compute the final overall outcome if not already explicitly set."""
        if error is not None:
            self.error = str(error)

        if self.build is not None and self.build.status == "FAILED":
            self.outcome = "BUILD_FAILED"
        elif (
            self.error is not None
            and not self.tests
            and (self.build is None or self.build.status != "FAILED")
        ):
            self.outcome = "ERROR"
        elif self.total_failed > 0:
            self.outcome = "FAILED"
        else:
            self.outcome = "PASSED"

    def to_dict(self) -> dict[str, typing.Any]:
        """Convert the summary to a JSON-serializable dictionary."""
        data: dict[str, typing.Any] = {
            "outcome": str(self.outcome),
            "total_passed": self.total_passed,
            "total_failed": self.total_failed,
            "total_skipped": self.total_skipped,
        }
        if self.summary_path is not None and isinstance(self.summary_path, str):
            data["summary_path"] = self.summary_path
        if self.error is not None and isinstance(self.error, str):
            data["error"] = self.error
        if self.build is not None:
            data["build"] = self.build.to_dict()
        if self.suggestions:
            data["suggestions"] = [s.to_dict() for s in self.suggestions]
        if self.hints:
            data["hints"] = [str(h) for h in self.hints]
        data["tests"] = [t.to_dict() for t in self.tests]
        return data

    def to_json(self, indent: int = 2) -> str:
        """Serialize the summary to a formatted JSON string."""
        return json.dumps(self.to_dict(), indent=indent)

    def write_to_file(self, target_path: str) -> None:
        """Write the summary JSON to disk, ensuring directory existence."""
        abs_path = os.path.abspath(target_path)
        self.summary_path = abs_path
        os.makedirs(os.path.dirname(abs_path), exist_ok=True)
        with open(abs_path, "w", encoding="utf-8") as f:
            json.dump(self.to_dict(), f, indent=2)
            f.write("\n")


def get_logs_dir(
    summary_json: str | None = None,
    artifact_output_directory: str | None = None,
) -> str | None:
    """Determine the base directory for isolated log files."""
    if summary_json:
        return os.path.join(
            os.path.dirname(os.path.abspath(summary_json)),
            "logs",
        )
    elif artifact_output_directory:
        return os.path.join(
            os.path.abspath(artifact_output_directory),
            "logs",
        )
    return None
