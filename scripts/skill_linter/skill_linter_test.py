#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import skill_linter


class TestSkillLinter(unittest.TestCase):
    def test_suggest_valid_name(self) -> None:
        self.assertEqual(
            skill_linter._suggest_valid_name("Valid-Name"), "valid-name"
        )
        self.assertEqual(
            skill_linter._suggest_valid_name("Name_With_Underscores"),
            "name-with-underscores",
        )
        self.assertEqual(
            skill_linter._suggest_valid_name("Invalid@Chars!"), "invalidchars"
        )
        self.assertEqual(skill_linter._suggest_valid_name("a" * 100), "a" * 64)

    def test_pre_process(self) -> None:
        # Test list item indentation standardization
        text = "-   item"
        self.assertEqual(skill_linter._pre_process(text, 4), "- item")

        text = "1.   item"
        self.assertEqual(skill_linter._pre_process(text, 2), "1. item")
        self.assertEqual(skill_linter._pre_process(text, 4), "1.  item")

        # Test that code blocks are ignored
        text = '```diff\ndeps = [\n-  "//src/lib/ddk"\n]\n```'
        self.assertEqual(skill_linter._pre_process(text, 4), text)

        text = "```markdown\n-   Item 1\n*   Item 2\n+   Item 3\n1.   Numbered\n```"
        self.assertEqual(skill_linter._pre_process(text, 4), text)

        text = "```markdown\n# Header 1\n## Header 2\n```"
        self.assertEqual(skill_linter._pre_process(text, 4), text)

    def test_post_process(self) -> None:
        # Test trailing whitespace removal and newlines
        text = "line with spaces    \nline 2"
        self.assertEqual(
            skill_linter._post_process(text), "line with spaces  \nline 2\n"
        )

        text = "line\n\n\nline2"
        self.assertEqual(skill_linter._post_process(text), "line\n\nline2\n")

    def test_validate_name(self) -> None:
        meta = {"name": "valid-name"}
        errors, fixes = skill_linter._validate_name(meta, fixit=False)
        self.assertEqual(errors, [])
        self.assertEqual(fixes, [])

        meta = {"name": "Invalid Name"}
        errors, fixes = skill_linter._validate_name(meta, fixit=False)
        self.assertTrue(len(errors) > 0)

        meta = {"name": "Invalid Name"}
        errors, fixes = skill_linter._validate_name(meta, fixit=True)
        self.assertEqual(meta["name"], "invalidname")
        self.assertTrue(len(fixes) > 0)

    def test_validate_description(self) -> None:
        meta = {"description": "Valid description"}
        errors, fixes = skill_linter._validate_description(meta, fixit=False)
        self.assertEqual(errors, [])
        self.assertEqual(fixes, [])

        meta = {"description": "Invalid description with <tags>"}
        errors, fixes = skill_linter._validate_description(meta, fixit=False)
        self.assertTrue(len(errors) > 0)

        meta = {"description": "Invalid description with <tags>"}
        errors, fixes = skill_linter._validate_description(meta, fixit=True)
        self.assertEqual(meta["description"], "Invalid description with")
        self.assertTrue(len(fixes) > 0)

    def test_format_markdown_paragraphs(self) -> None:
        # Test paragraph wrapping
        text = "This is a long paragraph that should be wrapped to fit within the eighty character limit that is imposed by the linter."
        formatted = skill_linter.format_markdown(text, width=40)
        expected = (
            "This is a long paragraph that should be\n"
            "wrapped to fit within the eighty\n"
            "character limit that is imposed by the\n"
            "linter.\n"
        )
        self.assertEqual(formatted, expected)

    def test_format_markdown_lists(self) -> None:
        # Test list formatting and indentation
        text = "- Item 1\n- Item 2 with a very long description that should wrap correctly under the marker."
        formatted = skill_linter.format_markdown(text, width=20)
        # Note: textwrap behavior might differ slightly from hand-calculated expected
        expected = (
            "- Item 1\n"
            "- Item 2 with a very\n"
            "  long description\n"
            "  that should wrap\n"
            "  correctly under\n"
            "  the marker.\n"
        )
        self.assertEqual(formatted, expected)

    def test_format_markdown_code_blocks(self) -> None:
        # Test that code blocks are preserved
        text = "Para before.\n\n```python\ndef foo():\n    pass\n```\n\nPara after."
        formatted = skill_linter.format_markdown(text)
        self.assertIn("```python\ndef foo():\n    pass\n```", formatted)

        # Test complex markdown structures inside a code block
        text = "```markdown\n-   item 1\n\n| col 1 | col 2 |\n|---|---|\n| a | b |\n\n# Header\n```"
        formatted = skill_linter.format_markdown(text)
        self.assertEqual(formatted, text + "\n")

        # Test preservation of purely whitespace lines in code blocks
        text = "```rust\nfn main() {\n    let x = 5;\n    \n    println!(x);\n}\n```"
        formatted = skill_linter.format_markdown(text)
        self.assertEqual(formatted, text + "\n")

    def test_format_markdown_tables(self) -> None:
        # Test that tables are preserved and not wrapped
        text = "| Header 1 | Header 2 |\n| --- | --- |\n| Cell 1 | Cell 2 |"
        formatted = skill_linter.format_markdown(text, width=10)
        # Width 10 should normally wrap lines, but tables should be skipped
        self.assertEqual(formatted, text + "\n")

    def test_format_markdown_headers(self) -> None:
        # Test that headers are preserved
        text = "# Header 1\n\n## Header 2\n\nContent."
        formatted = skill_linter.format_markdown(text)
        self.assertIn("# Header 1\n", formatted)
        self.assertIn("## Header 2\n", formatted)

    def test_format_markdown_hard_breaks(self) -> None:
        # Test that Markdown hard line breaks (two spaces) are preserved
        text = "Line 1  \nLine 2"
        formatted = skill_linter.format_markdown(text)
        self.assertEqual(formatted, "Line 1  \nLine 2\n")

    def test_format_markdown_with_issues_table_boundaries(self) -> None:
        # Test that lines immediately following a table are correctly validated for width
        text = "| Header |\n| --- |\n| Cell |\nThis is a very long text line immediately following the table with no newline."
        _, issues = skill_linter.format_markdown_with_issues(text, width=20)
        self.assertIn("lines exceeding 20 characters", issues)

    def test_format_markdown_with_issues_tables_ignored(self) -> None:
        text = "| Extremely long table header that exceeds eighty characters |\n| --- |\n| Cell without trailing spaces |\n"
        _, issues = skill_linter.format_markdown_with_issues(text, width=20)
        self.assertEqual(issues, [])

    def test_format_markdown_with_issues_code_blocks_ignored(self) -> None:
        text = "```python\ndef foo():\n\n\n    print('Trailing space here:   ')\n```\n"
        _, issues = skill_linter.format_markdown_with_issues(text, width=80)
        self.assertEqual(issues, [])

    def test_format_markdown_with_issues_normal_text(self) -> None:
        text = "Normal paragraph.\n\n\nAnother normal paragraph with trailing space    \n"
        _, issues = skill_linter.format_markdown_with_issues(text, width=80)
        self.assertIn("consecutive empty lines", issues)
        self.assertIn("trailing whitespace", issues)


if __name__ == "__main__":
    unittest.main()
