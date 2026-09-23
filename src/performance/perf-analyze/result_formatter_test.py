# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for the result_formatter module."""
import unittest
from typing import Any

from plugins import SectionResult
from result_formatter import FORMATTERS


class ResultFormatterTest(unittest.TestCase):
    """Tests for result formatters."""

    def test_format_raw_data(self) -> None:
        """Tests format_raw_data with normal tabular data."""
        test_cases: list[tuple[str, list[dict[str, Any]], str]] = [
            (
                "json",
                [{"col1": "val1", "col2": 2}],
                '[\n  {\n    "col1": "val1",\n    "col2": 2\n  }\n]',
            ),
            (
                "markdown",
                [{"col1": "val1", "col2": 2}],
                "| col1 | col2 |\n| --- | --- |\n| val1 | 2 |",
            ),
            (
                "text",
                [{"col1": "val1", "col2": 2}],
                "col1\tcol2\nval1\t2",
            ),
        ]
        for fmt_name, data, expected in test_cases:
            with self.subTest(format=fmt_name):
                formatter = FORMATTERS[fmt_name]
                output = formatter.format_raw_data(data)
                self.assertEqual(output.strip(), expected.strip())

    def test_format_raw_data_empty(self) -> None:
        """Tests format_raw_data with empty data."""
        test_cases: list[tuple[str, list[dict[str, Any]], str]] = [
            ("json", [], "[]"),
            ("markdown", [], "No results."),
            ("text", [], "No results."),
        ]
        for fmt_name, data, expected in test_cases:
            with self.subTest(format=fmt_name):
                formatter = FORMATTERS[fmt_name]
                output = formatter.format_raw_data(data)
                self.assertEqual(output.strip(), expected.strip())

    def test_format_raw_data_newlines(self) -> None:
        """Tests format_raw_data with newlines in data."""
        test_cases: list[tuple[str, list[dict[str, Any]], str]] = [
            (
                "json",
                [{"col1": "line1\nline2"}],
                '[\n  {\n    "col1": "line1\\nline2"\n  }\n]',
            ),
            (
                "markdown",
                [{"col1": "line1\nline2"}],
                "| col1 |\n| --- |\n| line1<br>line2 |",
            ),
            (
                "text",
                [{"col1": "line1\nline2"}],
                'col1\n"line1\nline2"',
            ),
        ]
        for fmt_name, data, expected in test_cases:
            with self.subTest(format=fmt_name):
                formatter = FORMATTERS[fmt_name]
                output = formatter.format_raw_data(data)
                self.assertEqual(output.strip(), expected.strip())

    def test_format_results_batch(self) -> None:
        """Tests format_results with batch SectionResult data."""
        batch_data: list[SectionResult] = [
            SectionResult(
                name="Query 1",
                results=[{"col1": "val1"}],
            ),
            SectionResult(
                name="Query 2",
                error="Some error occurred",
            ),
        ]
        test_cases: list[tuple[str, str]] = [
            (
                "json",
                '[\n  {\n    "name": "Query 1",\n    "results": [\n      {\n        "col1": "val1"\n      }\n    ]\n  },\n  {\n    "name": "Query 2",\n    "error": "Some error occurred"\n  }\n]',
            ),
            (
                "markdown",
                "### Query 1\n| col1 |\n| --- |\n| val1 |\n\n### Query 2\nError: Some error occurred",
            ),
            (
                "text",
                "Query: Query 1\ncol1\nval1\n\nQuery: Query 2\nError: Some error occurred",
            ),
        ]
        for fmt_name, expected in test_cases:
            with self.subTest(format=fmt_name):
                formatter = FORMATTERS[fmt_name]
                output = formatter.format_results(batch_data)
                self.assertEqual(output.strip(), expected.strip())

    def test_format_results_batch_with_note(self) -> None:
        """Tests format_results with batch data containing a note field."""
        batch_data: list[SectionResult] = [
            SectionResult(
                name="Query 1",
                note="A note explaining context",
                results=[{"col1": "val1"}],
            ),
        ]
        test_cases: list[tuple[str, str]] = [
            (
                "json",
                '[\n  {\n    "name": "Query 1",\n    "note": "A note explaining context",\n    "results": [\n      {\n        "col1": "val1"\n      }\n    ]\n  }\n]',
            ),
            (
                "markdown",
                "### Query 1\n> [!NOTE]\n> A note explaining context\n\n| col1 |\n| --- |\n| val1 |",
            ),
            (
                "text",
                "Query: Query 1\nNote: A note explaining context\ncol1\nval1",
            ),
        ]
        for fmt_name, expected in test_cases:
            with self.subTest(format=fmt_name):
                formatter = FORMATTERS[fmt_name]
                output = formatter.format_results(batch_data)
                self.assertEqual(output.strip(), expected.strip())

    def test_format_results_section_result_note_only(self) -> None:
        """Tests that a SectionResult with only a note does not output 'No results.'."""
        batch_data = [
            SectionResult(
                name="Advisory Notice",
                note="This is an informational notice.",
            ),
        ]
        md_output = FORMATTERS["markdown"].format_results(batch_data)
        self.assertEqual(
            md_output.strip(),
            "### Advisory Notice\n> [!NOTE]\n> This is an informational notice.",
        )
        text_output = FORMATTERS["text"].format_results(batch_data)
        self.assertEqual(
            text_output.strip(),
            "Query: Advisory Notice\nNote: This is an informational notice.",
        )

    def test_format_error(self) -> None:
        """Tests format_error."""
        test_cases: list[tuple[str, str, str]] = [
            ("json", "some error", '{\n  "error": "some error"\n}'),
            ("markdown", "some error", "**Error:** some error"),
            ("text", "some error", "Error: some error"),
        ]
        for fmt_name, error_msg, expected in test_cases:
            with self.subTest(format=fmt_name):
                formatter = FORMATTERS[fmt_name]
                output = formatter.format_error(error_msg)
                self.assertEqual(output.strip(), expected.strip())


if __name__ == "__main__":
    unittest.main()
