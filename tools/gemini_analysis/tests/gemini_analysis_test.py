#!/usr/bin/env fuchsia-vendored-python
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import MagicMock, mock_open, patch

# add the script's directory to the python path
sys.path.append("tools/gemini_analysis")
import gemini_analysis


class GeminiAnalysisTest(unittest.TestCase):
    @patch(
        "sys.argv",
        ["gemini_analysis.py", "--api-key", "test-key", "--verbosity", "1"],
    )
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.subprocess.run")
    def test_verbosity_1(
        self,
        mock_subprocess_run: MagicMock,
        mock_gemini_call: MagicMock,
    ) -> None:
        # setup mocks
        mock_gemini_call.return_value = gemini_analysis.GeminiAnalysisResult(
            text="path/to/file.rs:123"
        )
        sys.stdin = io.StringIO("mock error log with path/to/file.rs:123")
        captured_output = io.StringIO()
        with redirect_stdout(captured_output):
            gemini_analysis.main()

        output = captured_output.getvalue()
        self.assertIn("path/to/file.rs:123", output)
        mock_subprocess_run.assert_not_called()  # git diff should not be called

    @patch(
        "sys.argv",
        ["gemini_analysis.py", "--api-key", "test-key", "--verbosity", "2"],
    )
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.subprocess.run")
    def test_verbosity_2(
        self,
        mock_subprocess_run: MagicMock,
        mock_gemini_call: MagicMock,
    ) -> None:
        # setup mocks
        mock_subprocess_run.return_value.stdout = "mock git diff"
        mock_gemini_call.return_value = gemini_analysis.GeminiAnalysisResult(
            text="## KEY LINES\nkey line\n## POTENTIAL ERROR\nmock error"
        )
        sys.stdin = io.StringIO("mock error log")
        captured_output = io.StringIO()
        with redirect_stdout(captured_output):
            gemini_analysis.main()

        output = captured_output.getvalue()
        self.assertIn("--- Gemini Failure Analysis ---", output)
        self.assertIn("## POTENTIAL ERROR", output)
        mock_subprocess_run.assert_called_once()
        self.assertEqual(mock_gemini_call.call_count, 2)
        call_args = json.loads(mock_gemini_call.call_args_list[1].args[2])
        self.assertIn(
            "mock git diff", call_args["contents"][0]["parts"][0]["text"]
        )

    @patch(
        "sys.argv",
        ["gemini_analysis.py", "--api-key", "test-key", "--verbosity", "3"],
    )
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.os.path.getsize")
    @patch("gemini_analysis.os.path.isfile")
    @patch(
        "builtins.open", new_callable=mock_open, read_data=b"mock file content"
    )
    @patch("gemini_analysis.subprocess.run")
    def test_verbosity_3_successful_analysis_with_file_request(
        self,
        mock_subprocess_run: MagicMock,
        mock_open_file: MagicMock,
        mock_isfile: MagicMock,
        mock_getsize: MagicMock,
        mock_gemini_call: MagicMock,
    ) -> None:
        # setup mocks
        mock_subprocess_run.return_value.stdout = "mock git diff"
        mock_isfile.return_value = True
        mock_getsize.return_value = 100  # Mock file size to be under the limit
        mock_gemini_call.side_effect = [
            gemini_analysis.GeminiAnalysisResult(
                '{"primary_key_line": "mock_file.py"}'
            ),
            gemini_analysis.GeminiAnalysisResult('["mock_file.py"]'),
            gemini_analysis.GeminiAnalysisResult(
                "## ROOT CAUSE ANALYSIS\nThis is a mock analysis."
            ),
            gemini_analysis.GeminiAnalysisResult("default response"),
        ]
        sys.stdin = io.StringIO("mock error log")
        captured_output = io.StringIO()
        with redirect_stdout(captured_output):
            gemini_analysis.main()

        output = captured_output.getvalue()
        self.assertIn("--- Gemini Failure Analysis ---", output)
        self.assertIn("## ROOT CAUSE ANALYSIS", output)
        self.assertEqual(mock_gemini_call.call_count, 3)
        first_call_args = json.loads(mock_gemini_call.call_args_list[1].args[2])
        self.assertIn(
            "mock git diff", first_call_args["contents"][0]["parts"][0]["text"]
        )
        second_call_args = json.loads(
            mock_gemini_call.call_args_list[2].args[2]
        )
        self.assertIn(
            "Content of file 'mock_file.py':",
            second_call_args["contents"][0]["parts"][0]["text"],
        )

    @patch("logging.basicConfig")
    @patch(
        "sys.argv",
        ["gemini_analysis.py", "--api-key", "test-key", "--verbosity", "3"],
    )
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.os.path.getsize")
    @patch("gemini_analysis.os.path.isfile")
    @patch("builtins.open", new_callable=mock_open)
    @patch("gemini_analysis.subprocess.run")
    def test_verbosity_3_unreadable_file(
        self,
        mock_subprocess_run: MagicMock,
        mock_open_file: MagicMock,
        mock_isfile: MagicMock,
        mock_getsize: MagicMock,
        mock_gemini_call: MagicMock,
        mock_logging_config: MagicMock,
    ) -> None:
        # Mocks
        mock_subprocess_run.return_value.stdout = "mock git diff"
        mock_isfile.return_value = True
        mock_getsize.return_value = 100
        mock_open_file.side_effect = IOError("Permission denied")
        mock_gemini_call.side_effect = [
            gemini_analysis.GeminiAnalysisResult(
                '{"primary_key_line": "unreadable_file.txt"}'
            ),
            gemini_analysis.GeminiAnalysisResult('["unreadable_file.txt"]'),
            gemini_analysis.GeminiAnalysisResult(
                "## ROOT CAUSE ANALYSIS\nMock analysis."
            ),
            gemini_analysis.GeminiAnalysisResult("default response"),
        ]
        sys.stdin = io.StringIO("mock error log")

        # Capture stderr and stdout
        captured_output = io.StringIO()
        captured_error = io.StringIO()
        sys.stderr = captured_error

        with self.assertLogs(level="WARNING") as cm:
            with redirect_stdout(captured_output):
                gemini_analysis.main()

        # Assertions
        self.assertIn(
            "--- Gemini Failure Analysis ---", captured_output.getvalue()
        )
        self.assertIn(
            "Gemini analysis warning: Could not read file",
            captured_error.getvalue(),
        )
        self.assertTrue(any("Could not read file" in log for log in cm.output))

        # Check that the file content was not included in the final prompt
        second_call_args = json.loads(
            mock_gemini_call.call_args_list[1].args[2]
        )
        self.assertNotIn(
            "unreadable_file.txt",
            second_call_args["contents"][0]["parts"][0]["text"],
        )

    @patch("tempfile.NamedTemporaryFile")
    @patch("sys.argv", ["gemini_analysis.py", "--api-key", "test-key"])
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.subprocess.run")
    def test_api_error(
        self,
        mock_subprocess_run: MagicMock,
        mock_gemini_call: MagicMock,
        mock_named_temp_file: MagicMock,
    ) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            log_file = os.path.join(temp_dir, "gemini_analysis.log")
            mock_named_temp_file.return_value.__enter__.return_value.name = (
                log_file
            )

            # setup mocks
            mock_subprocess_run.return_value.stdout = ""
            mock_subprocess_run.return_value.stderr = ""
            mock_gemini_call.return_value = (
                gemini_analysis.GeminiAnalysisResult(
                    text="Error: API call failed", error=True
                )
            )
            sys.stdin = io.StringIO("mock error log")

            # capture stderr
            captured_error = io.StringIO()
            sys.stderr = captured_error

            # run and assert
            with self.assertRaises(SystemExit):
                gemini_analysis.main()

            self.assertIn("Error: API call failed", captured_error.getvalue())

            # check logs to ensure it logged the start but not the successful finish
            with open(log_file, "r") as f:
                log_output = f.read()
            self.assertIn("Starting Gemini analysis", log_output)
            self.assertIn("Error log from stdin", log_output)
            self.assertNotIn("Final analysis output", log_output)

    @patch(
        "sys.argv",
        [
            "gemini_analysis.py",
            "--api-key",
            "test-key",
            "--gemini-model",
            "test-model",
        ],
    )
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.subprocess.run")
    def test_gemini_model_cli_arg(
        self,
        mock_subprocess_run: MagicMock,
        mock_gemini_call: MagicMock,
    ) -> None:
        # setup mocks
        mock_subprocess_run.return_value.stdout = ""
        mock_subprocess_run.return_value.stderr = ""
        mock_gemini_call.return_value = gemini_analysis.GeminiAnalysisResult(
            text="mock response"
        )
        sys.stdin = io.StringIO("mock error log")
        with redirect_stdout(io.StringIO()):
            gemini_analysis.main()
        # check that the correct model was used
        self.assertEqual(mock_gemini_call.call_count, 2)
        self.assertEqual(mock_gemini_call.call_args[0][1], "test-model")

    @patch("tempfile.NamedTemporaryFile")
    @patch("sys.argv", ["gemini_analysis.py", "--api-key", "test-key"])
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.subprocess.run")
    def test_log_file_overwrite(
        self,
        mock_run: MagicMock,
        mock_gemini_call: MagicMock,
        mock_named_temp_file: MagicMock,
    ) -> None:
        """tests that the log file is overwritten on subsequent runs."""
        with tempfile.TemporaryDirectory() as temp_dir:
            log_file = os.path.join(temp_dir, "gemini_analysis.log")
            mock_named_temp_file.return_value.__enter__.return_value.name = (
                log_file
            )

            # setup mocks
            mock_gemini_call.return_value = (
                gemini_analysis.GeminiAnalysisResult(text="first call")
            )
            sys.stdin = io.StringIO("mock error log")

            # run main once
            with redirect_stdout(io.StringIO()):
                gemini_analysis.main()

            # check log content
            with open(log_file, "r") as f:
                content = f.read()
            self.assertIn("first call", content)

            # setup mocks for second call
            mock_gemini_call.return_value = (
                gemini_analysis.GeminiAnalysisResult(text="second call")
            )
            sys.stdin = io.StringIO("another mock error log")

            # run main again
            with redirect_stdout(io.StringIO()):
                gemini_analysis.main()

            # check log content again
            with open(log_file, "r") as f:
                content = f.read()
            self.assertIn("second call", content)
            self.assertNotIn("first call", content)

    @patch("tempfile.NamedTemporaryFile")
    @patch("gemini_analysis._blocking_gemini_call")
    @patch("gemini_analysis.subprocess.run")
    @patch(
        "sys.argv",
        ["gemini_analysis.py", "--api-key", "test-key", "--verbosity", "2"],
    )
    def test_log_file_receives_content(
        self,
        mock_subprocess_run: MagicMock,
        mock_gemini_call: MagicMock,
        mock_named_temp_file: MagicMock,
    ) -> None:
        """tests that log messages from main() are written to the file."""
        with tempfile.TemporaryDirectory() as temp_dir:
            log_file = os.path.join(temp_dir, "gemini_analysis.log")
            mock_named_temp_file.return_value.__enter__.return_value.name = (
                log_file
            )

            # setup mocks
            mock_subprocess_run.return_value.stdout = "mock git diff"
            mock_gemini_call.return_value = (
                gemini_analysis.GeminiAnalysisResult(text="mock response")
            )
            sys.stdin = io.StringIO("mock error log")

            # run main
            gemini_analysis.main()

            # read the log file and check for expected content
            self.assertTrue(os.path.exists(log_file))
            with open(log_file, "r") as f:
                content = f.read()

            self.assertIn("Starting Gemini analysis", content)
            self.assertIn("Git diff", content)
            self.assertIn("Error log from stdin", content)
            self.assertIn("Final analysis output", content)

    @patch("gemini_analysis._print_colorized_log")
    @patch(
        "sys.argv",
        ["gemini_analysis.py", "--api-key", "test-key", "--verbosity", "1"],
    )
    @patch("gemini_analysis._blocking_gemini_call")
    def test_printed_annotation(
        self,
        mock_gemini_call: MagicMock,
        mock_print_colorized_log: MagicMock,
    ) -> None:
        # setup mocks
        mock_gemini_call.return_value = gemini_analysis.GeminiAnalysisResult(
            text='{"primary_key_line": "key line"}'
        )
        sys.stdin = io.StringIO("mock error log with key line")
        gemini_analysis.main()

        # check that the print function was called correctly
        mock_print_colorized_log.assert_called_once_with(
            "mock error log with key line", '{"primary_key_line": "key line"}'
        )

    def test_colorization_output(self) -> None:
        error_log = "line 1\nkey line\nline 3"
        annotation = '{"primary_key_line": "key line"}'

        # ANSI color codes
        COLOR_RED = "\033[91m"
        COLOR_RESET = "\033[0m"

        expected_output = (
            "line 1\n" f"{COLOR_RED}key line{COLOR_RESET}\n" "line 3\n\n"
        )

        captured_output = io.StringIO()
        with redirect_stdout(captured_output):
            gemini_analysis._print_colorized_log(error_log, annotation)

        self.assertEqual(captured_output.getvalue(), expected_output)


if __name__ == "__main__":
    unittest.main()
