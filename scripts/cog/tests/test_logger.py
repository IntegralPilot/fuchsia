# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from unittest.mock import patch

import logger


class TestLogger(unittest.TestCase):
    def tearDown(self) -> None:
        if logger._logger:
            if logger._file_handler:
                logger._logger.removeHandler(logger._file_handler)
                logger._file_handler.close()
                logger._file_handler = None
            if logger._memory_handler:
                logger._logger.removeHandler(logger._memory_handler)
                logger._memory_handler.close()
                logger._memory_handler = None
        super().tearDown()

    @patch("builtins.print")
    def test_emit_status(self, mock_print: unittest.mock.MagicMock) -> None:
        # Enable status updates and verify that emit_status() prints to stdout.
        logger.init_logger(enable_status_updates=True)
        logger.emit_status("test message")
        mock_print.assert_called_once_with("STATUS_UPDATE:test message")

        # Disable status updates and verify that emit_status() does not print.
        mock_print.reset_mock()
        logger.init_logger(enable_status_updates=False)
        logger.emit_status("another message")
        mock_print.assert_not_called()

        # Re-enable status updates to ensure it still works.
        mock_print.reset_mock()
        logger.init_logger(enable_status_updates=True)
        logger.emit_status("third message")
        mock_print.assert_called_once_with("STATUS_UPDATE:third message")

    def test_set_level(self) -> None:
        import logging

        logger.init_logger(log_level=logging.WARNING)
        self.assertEqual(logger.get_log_level(), logging.WARNING)

        with logger.set_level(logging.DEBUG):
            self.assertEqual(logger.get_log_level(), logging.DEBUG)

        self.assertEqual(logger.get_log_level(), logging.WARNING)

        # Test with min
        with logger.set_level(min(logger.get_log_level(), logging.INFO)):
            self.assertEqual(logger.get_log_level(), logging.INFO)

        logger.init_logger(log_level=logging.DEBUG)
        with logger.set_level(min(logger.get_log_level(), logging.INFO)):
            self.assertEqual(logger.get_log_level(), logging.DEBUG)

    def test_log_exception_normal(self) -> None:
        import logging

        logger.init_logger(log_level=logging.WARNING)

        with self.assertLogs("cog", level="ERROR") as cm:
            try:
                raise ValueError("test error")
            except ValueError:
                logger.log_exception("An error occurred")

        self.assertEqual(len(cm.records), 2)
        for record in cm.records:
            self.assertEqual(record.funcName, "test_log_exception_normal")
            self.assertEqual(record.filename, "test_logger.py")

        self.assertEqual(cm.records[0].getMessage(), "An error occurred")
        self.assertEqual(cm.records[1].getMessage(), "test error")

    def test_log_exception_normal_debug(self) -> None:
        import logging

        logger.init_logger(log_level=logging.DEBUG)

        with self.assertLogs("cog", level="DEBUG") as cm:
            try:
                raise ValueError("test error")
            except ValueError:
                logger.log_exception("An error occurred")

        self.assertEqual(len(cm.records), 1)
        record = cm.records[0]
        self.assertEqual(record.funcName, "test_log_exception_normal_debug")
        self.assertEqual(record.filename, "test_logger.py")
        self.assertEqual(record.getMessage(), "An error occurred")
        self.assertEqual(record.levelno, logging.ERROR)

    def test_log_exception_called_process_error(self) -> None:
        import logging
        import subprocess

        logger.init_logger(log_level=logging.WARNING)

        e = subprocess.CalledProcessError(
            1, ["ls", "dir"], output=b"out", stderr=b"err"
        )
        with self.assertLogs("cog", level="ERROR") as cm:
            try:
                raise e
            except subprocess.CalledProcessError:
                logger.log_exception("Command failed")

        self.assertEqual(len(cm.records), 4)
        for record in cm.records:
            self.assertEqual(
                record.funcName, "test_log_exception_called_process_error"
            )
            self.assertEqual(record.filename, "test_logger.py")

        self.assertEqual(cm.records[0].getMessage(), "Command failed")
        self.assertEqual(
            cm.records[1].getMessage(), "Command `ls dir` exited with status 1"
        )
        self.assertEqual(cm.records[2].getMessage(), "stdout: out")
        self.assertEqual(cm.records[3].getMessage(), "stderr: err")

    def test_log_exception_called_process_error_debug(self) -> None:
        import logging
        import subprocess

        logger.init_logger(log_level=logging.DEBUG)

        e = subprocess.CalledProcessError(
            1, ["ls", "dir"], output=b"out", stderr=b"err"
        )
        with self.assertLogs("cog", level="DEBUG") as cm:
            try:
                raise e
            except subprocess.CalledProcessError:
                logger.log_exception("Command failed")

        self.assertEqual(len(cm.records), 4)
        for record in cm.records:
            self.assertEqual(
                record.funcName, "test_log_exception_called_process_error_debug"
            )
            self.assertEqual(record.filename, "test_logger.py")

        self.assertEqual(cm.records[0].getMessage(), "Command failed")
        self.assertEqual(cm.records[0].levelno, logging.ERROR)
        self.assertEqual(
            cm.records[1].getMessage(), "Command `ls dir` exited with status 1"
        )
        self.assertEqual(cm.records[2].getMessage(), "stdout: out")
        self.assertEqual(cm.records[3].getMessage(), "stderr: err")

    def test_retroactive_file_logging(self) -> None:
        import logging
        import tempfile
        from pathlib import Path

        # Reset the logger and stream/file/memory handlers
        logger.init_logger(log_level=logging.DEBUG)

        # Log messages before file logging is set up
        logger.log_info("Initial info message")
        logger.log_debug("Initial debug message")

        with tempfile.TemporaryDirectory() as tmpdir:
            workspace_path = Path(tmpdir)
            log_file = workspace_path / "workspace_setup.log"

            # Verify file doesn't exist yet
            self.assertFalse(log_file.exists())

            # Initialize file logging, which should retroactively dump prior logs
            logger.setup_file_logging(workspace_path)

            # Log a message after file logging is set up
            logger.log_info("Message after file setup")

            # Verify file now exists
            self.assertTrue(log_file.exists())

            # Read log content
            log_content = log_file.read_text()

            self.assertIn("Initial info message", log_content)
            self.assertIn("Initial debug message", log_content)
            self.assertIn("Message after file setup", log_content)

    def test_get_log_path_before_file_setup(self) -> None:
        import logging
        import os

        logger.init_logger(log_level=logging.DEBUG)
        logger.log_info("Test log entry before file setup")

        temp_log_path = logger.get_log_path()
        self.addCleanup(
            lambda: os.remove(temp_log_path) if temp_log_path.exists() else None
        )
        self.assertTrue(temp_log_path.is_absolute())
        self.assertTrue(temp_log_path.exists())

        log_content = temp_log_path.read_text()
        self.assertIn("Test log entry before file setup", log_content)

        # Log another entry and call get_log_path again
        logger.log_info("Another test log entry")
        temp_log_path_2 = logger.get_log_path()
        self.addCleanup(
            lambda: os.remove(temp_log_path_2)
            if temp_log_path_2.exists()
            else None
        )
        self.assertTrue(temp_log_path_2.is_absolute())
        self.assertTrue(temp_log_path_2.exists())
        # The two paths should be different
        self.assertNotEqual(temp_log_path, temp_log_path_2)

        log_content_2 = temp_log_path_2.read_text()
        self.assertIn("Test log entry before file setup", log_content_2)
        self.assertIn("Another test log entry", log_content_2)

    def test_get_log_path_after_file_setup(self) -> None:
        import logging
        import os
        import tempfile
        from pathlib import Path

        logger.init_logger(log_level=logging.DEBUG)

        with tempfile.TemporaryDirectory() as tmpdir:
            workspace_path = Path(tmpdir)
            logger.setup_file_logging(workspace_path)

            log_path = logger.get_log_path()
            # The path should be relative to CWD
            self.assertFalse(log_path.is_absolute())

            # Verify it matches the relative path of workspace_setup.log to CWD
            expected_path = Path(
                os.path.relpath(workspace_path / "workspace_setup.log")
            )
            self.assertEqual(log_path, expected_path)


if __name__ == "__main__":
    unittest.main()
