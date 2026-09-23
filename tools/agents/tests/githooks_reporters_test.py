#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for the git hooks reporters."""

from __future__ import annotations

import io
import unittest
from typing import TextIO, cast

from agents.lib.githooks.reporters import (
    ConsoleReporter,
    _is_utf8_supported,
)


class ConsoleReporterTest(unittest.TestCase):
    """Unit tests for ConsoleReporter."""

    def test_unicode_mode_symbols_and_messages(self) -> None:
        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(
            stdout_stream=stdout_buf,
            stderr_stream=stderr_buf,
            unicode=True,
        )

        self.assertFalse(reporter.has_errors)

        reporter.on_files_restaged(
            ["foo.py", "bar.cc"],
            message="Auto-formatted and re-staged",
        )
        self.assertIn(
            "🧹 Auto-formatted and re-staged: foo.py, bar.cc",
            stdout_buf.getvalue(),
        )

        reporter.on_warning("Subject exceeds 50 chars\n  - details")
        self.assertIn(
            "⚠️ Subject exceeds 50 chars\n  - details",
            stderr_buf.getvalue(),
        )

        reporter.on_partial_staging_conflict(
            ["baz.py"],
            message="Formatting issues in partially staged files",
        )
        stderr_val = stderr_buf.getvalue()
        self.assertIn(
            "❌ Formatting issues in partially staged files", stderr_val
        )

        reporter.on_error("Something went wrong")
        self.assertIn("❌ Something went wrong", stderr_buf.getvalue())
        self.assertTrue(reporter.has_errors)

    def test_on_files_restaged_when_filename_is_substring_of_message(
        self,
    ) -> None:
        stdout_buf = io.StringIO()
        reporter = ConsoleReporter(
            stdout_stream=stdout_buf,
            unicode=False,
        )
        # "bar.py" is a substring of "Re-staged file bar.py"
        reporter.on_files_restaged(
            ["bar.py"],
            message="Re-staged file bar.py",
        )
        output = stdout_buf.getvalue()
        self.assertIn(
            "[FIXED] Re-staged file bar.py: bar.py",
            output,
        )

    def test_ascii_mode_symbols(self) -> None:
        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(
            stdout_stream=stdout_buf,
            stderr_stream=stderr_buf,
            unicode=False,
        )

        reporter.on_files_restaged(
            ["foo.py"],
            message="Auto-formatted and re-staged",
        )
        self.assertIn(
            "[FIXED] Auto-formatted and re-staged: foo.py",
            stdout_buf.getvalue(),
        )

        reporter.on_partial_staging_conflict(
            ["foo.py"],
            message="Formatting issues in partially staged files",
        )
        stderr_val = stderr_buf.getvalue()
        self.assertIn(
            "[ERROR] Formatting issues in partially staged files", stderr_val
        )

        reporter.on_error("Something went wrong")
        self.assertIn("[ERROR] Something went wrong", stderr_buf.getvalue())

    def test_remediation_command(self) -> None:
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(
            stderr_stream=stderr_buf,
            unicode=False,
        )
        reporter.on_partial_staging_conflict(
            ["file1.py"],
            message="Formatting issues in partially staged files",
            remediation_cmds=["fx format-code --files=file1.py"],
        )
        val = stderr_buf.getvalue()
        self.assertIn("Or fix directly:", val)
        self.assertIn("fx format-code --files=file1.py", val)

    def test_partial_staging_conflict_empty_message_fallback(self) -> None:
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(
            stderr_stream=stderr_buf,
            unicode=False,
        )
        reporter.on_partial_staging_conflict(
            ["conflict.py"],
            message="",
        )
        val = stderr_buf.getvalue()
        self.assertIn("[ERROR] Partially staged files failed checks", val)
        self.assertEqual(
            reporter.errors,
            ["Partially staged files failed checks"],
        )

    def test_partial_staging_conflict_empty_files_and_message(self) -> None:
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(
            stderr_stream=stderr_buf,
            unicode=False,
        )
        reporter.on_partial_staging_conflict([], "")
        val = stderr_buf.getvalue()
        self.assertIn("[ERROR] Partially staged files failed checks", val)
        self.assertEqual(
            reporter.errors,
            ["Partially staged files failed checks"],
        )

    def test_on_error_and_on_warning_stream_all_messages(self) -> None:
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(stderr_stream=stderr_buf, unicode=True)
        reporter.on_error("Duplicate error message")
        reporter.on_error("Duplicate error message")
        reporter.on_warning("Duplicate warning message")
        reporter.on_warning("Duplicate warning message")
        self.assertEqual(
            reporter.errors,
            ["Duplicate error message", "Duplicate error message"],
        )
        self.assertEqual(
            reporter.warnings,
            ["Duplicate warning message", "Duplicate warning message"],
        )
        self.assertEqual(
            stderr_buf.getvalue(),
            "❌ Duplicate error message\n"
            "❌ Duplicate error message\n"
            "⚠️ Duplicate warning message\n"
            "⚠️ Duplicate warning message\n",
        )

    def test_unicode_message_on_ascii_stream_does_not_raise(self) -> None:
        stdout_raw = io.BytesIO()
        stderr_raw = io.BytesIO()
        stdout_stream = io.TextIOWrapper(
            stdout_raw, encoding="ascii", errors="strict"
        )
        stderr_stream = io.TextIOWrapper(
            stderr_raw, encoding="ascii", errors="strict"
        )

        reporter = ConsoleReporter(
            stdout_stream=stdout_stream,
            stderr_stream=stderr_stream,
            unicode=True,
        )

        reporter.on_files_restaged(["file.py"], message="Cleaned ✨")
        reporter.on_warning("Warning with unicode: ⚠️ and résumé")
        reporter.on_error("Error with unicode: ❌ and naïve")
        reporter.on_partial_staging_conflict(
            ["conflict.py"], message="Conflict with 💥"
        )

        stdout_stream.flush()
        stderr_stream.flush()
        self.assertIn("Cleaned", stdout_raw.getvalue().decode("ascii"))
        self.assertIn(
            "Warning with unicode", stderr_raw.getvalue().decode("ascii")
        )
        self.assertIn(
            "Error with unicode", stderr_raw.getvalue().decode("ascii")
        )
        self.assertIn("Conflict with", stderr_raw.getvalue().decode("ascii"))

    def test_stream_encoding_asymmetry_degrades_safely(self) -> None:
        class MockStream:
            def __init__(self, encoding: str) -> None:
                self.buf = io.StringIO()
                self.encoding = encoding

            def isatty(self) -> bool:
                return True

            def write(self, s: str) -> int:
                return self.buf.write(s)

            def getvalue(self) -> str:
                return self.buf.getvalue()

            def flush(self) -> None:
                self.buf.flush()

        stdout_utf8 = MockStream("utf-8")
        stderr_ascii = MockStream("ascii")
        reporter = ConsoleReporter(
            stdout_stream=cast(TextIO, stdout_utf8),
            stderr_stream=cast(TextIO, stderr_ascii),
        )

        reporter.on_files_restaged(
            ["test.py"], message="Auto-formatted and re-staged"
        )
        reporter.on_warning("Warning test")
        reporter.on_partial_staging_conflict(
            ["test.py"],
            message="Conflict test",
        )
        self.assertIn("🧹", stdout_utf8.getvalue())
        self.assertNotIn("[FIXED]", stdout_utf8.getvalue())
        self.assertIn("[WARN]", stderr_ascii.getvalue())
        self.assertIn("[ERROR]", stderr_ascii.getvalue())
        self.assertNotIn("⚠️", stderr_ascii.getvalue())
        self.assertNotIn("❌", stderr_ascii.getvalue())

    def test_messages_preserve_prefixes(self) -> None:
        test_cases = [
            (
                True,
                "[warning] Warning with bracket prefix",
                "⚠️ [warning] Warning with bracket prefix\n",
            ),
            (
                False,
                "Error: Header\n    indented code\n  preserved",
                "❌ Error: Header\n    indented code\n  preserved\n",
            ),
        ]
        for is_warn, msg, expected in test_cases:
            stderr_buf = io.StringIO()
            reporter = ConsoleReporter(stderr_stream=stderr_buf, unicode=True)
            if is_warn:
                reporter.on_warning(msg)
            else:
                reporter.on_error(msg)
            self.assertEqual(stderr_buf.getvalue(), expected)

    def test_finish_flushes_streams(self) -> None:
        class FlushingBuffer(io.StringIO):
            def __init__(self) -> None:
                super().__init__()
                self.flushed = False

            def flush(self) -> None:
                self.flushed = True

        out_buf = FlushingBuffer()
        err_buf = FlushingBuffer()
        reporter = ConsoleReporter(
            stdout_stream=out_buf,
            stderr_stream=err_buf,
        )
        reporter.finish()
        self.assertTrue(out_buf.flushed)
        self.assertTrue(err_buf.flushed)

    def test_whitespace_only_errors_and_warnings(self) -> None:
        """Verifies whitespace-only errors and warnings are suppressed and not recorded."""
        stderr_buf = io.StringIO()
        reporter = ConsoleReporter(stderr_stream=stderr_buf)
        reporter.on_error("   \n  ")
        reporter.on_warning(" \t ")
        self.assertEqual(stderr_buf.getvalue(), "")
        self.assertFalse(reporter.has_errors)
        self.assertEqual(reporter.errors, [])
        self.assertEqual(reporter.warnings, [])

    def test_unicode_tty_detection(self) -> None:
        """Verifies decoupled TTY and unicode detection between stdout and stderr."""

        class MockNonTtyStream(io.StringIO):
            def isatty(self) -> bool:
                return False

        class MockTtyStream(io.StringIO):
            def isatty(self) -> bool:
                return True

        stdout_buf = MockNonTtyStream()
        stderr_buf = MockTtyStream()
        reporter = ConsoleReporter(
            stdout_stream=stdout_buf,
            stderr_stream=stderr_buf,
            unicode=None,
        )
        reporter.on_files_restaged(
            ["file.py"], message="Auto-formatted and re-staged"
        )
        reporter.on_warning("A warning message")
        reporter.on_error("An error message")
        self.assertIn("[FIXED]", stdout_buf.getvalue())
        self.assertNotIn("🧹", stdout_buf.getvalue())
        self.assertIn("⚠️", stderr_buf.getvalue())
        self.assertIn("❌", stderr_buf.getvalue())
        self.assertNotIn("[WARN]", stderr_buf.getvalue())
        self.assertNotIn("[ERROR]", stderr_buf.getvalue())

    def test_unicode_tty_detection_both_tty(self) -> None:
        """Verifies that if both streams are TTY, both receive unicode."""

        class MockTtyStream(io.StringIO):
            def isatty(self) -> bool:
                return True

        stdout_buf = MockTtyStream()
        stderr_buf = MockTtyStream()
        reporter = ConsoleReporter(
            stdout_stream=stdout_buf,
            stderr_stream=stderr_buf,
            unicode=None,
        )
        reporter.on_files_restaged(
            ["file.py"], message="Auto-formatted and re-staged"
        )
        reporter.on_warning("A warning message")
        reporter.on_error("An error message")
        self.assertIn("🧹", stdout_buf.getvalue())
        self.assertNotIn("[FIXED]", stdout_buf.getvalue())
        self.assertIn("⚠️", stderr_buf.getvalue())
        self.assertIn("❌", stderr_buf.getvalue())
        self.assertNotIn("[WARN]", stderr_buf.getvalue())
        self.assertNotIn("[ERROR]", stderr_buf.getvalue())

    def test_unicode_tty_detection_both_non_tty(self) -> None:
        """Verifies that if both streams are non-TTY, both receive ASCII."""

        class MockNonTtyStream(io.StringIO):
            def isatty(self) -> bool:
                return False

        stdout_buf = MockNonTtyStream()
        stderr_buf = MockNonTtyStream()
        reporter = ConsoleReporter(
            stdout_stream=stdout_buf,
            stderr_stream=stderr_buf,
            unicode=None,
        )
        reporter.on_files_restaged(
            ["file.py"], message="Auto-formatted and re-staged"
        )
        reporter.on_warning("A warning message")
        reporter.on_error("An error message")
        self.assertIn("[FIXED]", stdout_buf.getvalue())
        self.assertNotIn("🧹", stdout_buf.getvalue())
        self.assertIn("[WARN]", stderr_buf.getvalue())
        self.assertIn("[ERROR]", stderr_buf.getvalue())
        self.assertNotIn("⚠️", stderr_buf.getvalue())
        self.assertNotIn("❌", stderr_buf.getvalue())

    def test_is_utf8_supported(self) -> None:
        """Verifies UTF-8 capability probe checks 🧹, ⚠️, and ❌."""

        class MockStream:
            def __init__(self, encoding: str | None) -> None:
                self.encoding = encoding

        self.assertTrue(_is_utf8_supported(MockStream("utf-8")))
        self.assertTrue(_is_utf8_supported(MockStream(None)))
        self.assertFalse(_is_utf8_supported(MockStream("ascii")))
        self.assertFalse(_is_utf8_supported(MockStream("invalid-encoding-xyz")))


if __name__ == "__main__":
    unittest.main()
