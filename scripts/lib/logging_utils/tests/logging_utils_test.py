# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for logging_utils module."""

from __future__ import annotations

import io
import logging
import tempfile
import unittest
from unittest import mock

import logging_utils


class LoggingUtilsTest(unittest.TestCase):
    """Unit tests for logging_utils module."""

    def test_supports_color_true_when_tty(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = True
        with mock.patch.dict("os.environ", {"TERM": "xterm"}, clear=True):
            self.assertTrue(logging_utils.supports_color(mock_stream))

    def test_supports_color_false_when_no_tty(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = False
        with mock.patch.dict("os.environ", {"TERM": "xterm"}, clear=True):
            self.assertFalse(logging_utils.supports_color(mock_stream))

    def test_supports_color_false_with_no_color_env(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = True
        with mock.patch.dict("os.environ", {"NO_COLOR": "1"}):
            self.assertFalse(logging_utils.supports_color(mock_stream))

    def test_supports_color_false_with_dumb_term(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = True
        with mock.patch.dict("os.environ", {"TERM": "dumb"}):
            self.assertFalse(logging_utils.supports_color(mock_stream))

    def test_supports_color_true_with_force_color(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = False
        with mock.patch.dict("os.environ", {"FORCE_COLOR": "1"}, clear=True):
            self.assertTrue(logging_utils.supports_color(mock_stream))

    def test_supports_color_true_with_clicolor_force(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = False
        with mock.patch.dict("os.environ", {"CLICOLOR_FORCE": "1"}, clear=True):
            self.assertTrue(logging_utils.supports_color(mock_stream))

    def test_supports_color_true_with_fuchsia_fx_test_run(self) -> None:
        mock_stream = mock.MagicMock()
        mock_stream.isatty.return_value = False
        with mock.patch.dict(
            "os.environ", {"FUCHSIA_FX_TEST_RUN": "1"}, clear=True
        ):
            self.assertTrue(logging_utils.supports_color(mock_stream))

    def test_colored_formatter_without_color(self) -> None:
        formatter = logging_utils.ColoredFormatter(
            "%(levelname)s %(message)s",
            prefix="[MOCK_PREFIX] ",
            use_color=False,
        )
        record = logging.LogRecord(
            name="test",
            level=logging.INFO,
            pathname="test.py",
            lineno=1,
            msg="[Test] test_case PASS [ret: 0]",
            args=(),
            exc_info=None,
        )
        output = formatter.format(record)
        self.assertEqual(
            output, "[MOCK_PREFIX] INFO [Test] test_case PASS [ret: 0]"
        )

    def test_colored_formatter_with_color_results(self) -> None:
        formatter = logging_utils.ColoredFormatter(
            "[MOCK_PREFIX] %(levelname)s %(message)s",
            use_color=True,
        )
        for result in ("PASS", "FAIL", "ERROR", "SKIP"):
            record = logging.LogRecord(
                name="test",
                level=logging.INFO,
                pathname="test.py",
                lineno=1,
                msg=f"[Test] test_case {result}",
                args=(),
                exc_info=None,
            )
            output = formatter.format(record)
            self.assertIn("\033[", output)
            self.assertIn(result, output)
            # Ensure levelname was restored on the record
            self.assertEqual(record.levelname, "INFO")

    def test_colored_formatter_return_codes(self) -> None:
        formatter = logging_utils.ColoredFormatter(
            "%(message)s",
            use_color=True,
        )
        # Success code (0)
        record_pass = logging.LogRecord(
            name="test",
            level=logging.INFO,
            pathname="test.py",
            lineno=1,
            msg="Completed: test_case PASS [ret: 0]",
            args=(),
            exc_info=None,
        )
        out_pass = formatter.format(record_pass)
        self.assertIn("ret: 0", out_pass)
        self.assertIn("\033[32mret: 0", out_pass)  # Green

        # Failure code (1)
        record_fail = logging.LogRecord(
            name="test",
            level=logging.ERROR,
            pathname="test.py",
            lineno=1,
            msg="Completed: test_case FAIL [ret: 1]",
            args=(),
            exc_info=None,
        )
        out_fail = formatter.format(record_fail)
        self.assertIn("ret: 1", out_fail)
        self.assertIn("\033[31mret: 1", out_fail)  # Red

    def test_colored_formatter_target_names(self) -> None:
        formatter = logging_utils.ColoredFormatter(
            "%(message)s",
            use_color=True,
        )
        record = logging.LogRecord(
            name="test",
            level=logging.INFO,
            pathname="test.py",
            lineno=1,
            msg="Connecting to fuchsia-1a7d-1303-a909 and fuchsia-emulator",
            args=(),
            exc_info=None,
        )
        out = formatter.format(record)
        self.assertIn("\033[36mfuchsia-1a7d-1303-a909\033[0m", out)
        self.assertIn("\033[36mfuchsia-emulator\033[0m", out)

    def test_colorize_logger_preserves_file_handler(self) -> None:
        test_logger = logging.Logger("test_logger")
        stream = io.StringIO()
        stream_handler = logging.StreamHandler(stream)
        test_logger.addHandler(stream_handler)

        with tempfile.NamedTemporaryFile() as tmp_file:
            file_handler = logging.FileHandler(tmp_file.name)
            original_file_formatter = logging.Formatter("%(message)s")
            file_handler.setFormatter(original_file_formatter)
            test_logger.addHandler(file_handler)

            logging_utils.colorize_logger(test_logger, prefix="PREFIX")

            # Stream handler should be colorized
            self.assertIsInstance(
                stream_handler.formatter, logging_utils.ColoredFormatter
            )
            # File handler should remain un-colorized
            self.assertIs(file_handler.formatter, original_file_formatter)
            self.assertNotIsInstance(
                file_handler.formatter, logging_utils.ColoredFormatter
            )

    def test_colored_formatter_with_color_and_prefix(self) -> None:
        formatter = logging_utils.ColoredFormatter(
            "%(levelname)s %(message)s",
            prefix="[MY_PREFIX] ",
            use_color=True,
        )
        record = logging.LogRecord(
            name="test",
            level=logging.INFO,
            pathname="test.py",
            lineno=1,
            msg="hello",
            args=(),
            exc_info=None,
        )
        output = formatter.format(record)
        self.assertTrue(output.startswith("[MY_PREFIX] "))
        self.assertIn("hello", output)


if __name__ == "__main__":
    unittest.main()
