# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for mobly_logger_extension module."""

from __future__ import annotations

import logging
import tempfile
import unittest
from unittest import mock

import logging_utils
import mobly_logger_extension
from mobly import logger as mobly_logger


class MoblyLoggerExtensionTest(unittest.TestCase):
    """Unit tests for mobly_logger_extension."""

    def tearDown(self) -> None:
        mobly_logger.kill_test_logger(logging.getLogger())
        super().tearDown()

    def test_mobly_logger_is_patched(self) -> None:
        setup_fn = getattr(mobly_logger, "_setup_test_logger", None)
        self.assertIsNotNone(setup_fn)
        self.assertTrue(getattr(setup_fn, "_is_color_patched", False))

    def test_setup_test_logger_invokes_colorize_logger(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            with mock.patch.object(
                logging_utils,
                "colorize_logger",
                wraps=logging_utils.colorize_logger,
            ) as mock_colorize:
                mobly_logger.setup_test_logger(
                    tmp_dir, prefix="TEST_PREFIX", console_level=logging.INFO
                )
                mock_colorize.assert_called_once_with(
                    logging.getLogger(),
                    prefix="TEST_PREFIX",
                    format_func=logging_utils.lacewing_log_formatter,
                )

    def test_setup_test_logger_colorizes_console_handler_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            mobly_logger.setup_test_logger(
                tmp_dir, prefix="TEST_PREFIX", console_level=logging.INFO
            )
            root_logger = logging.getLogger()

            stream_handlers = [
                h
                for h in root_logger.handlers
                if isinstance(h, logging.StreamHandler)
                and not isinstance(h, logging.FileHandler)
            ]
            file_handlers = [
                h
                for h in root_logger.handlers
                if isinstance(h, logging.FileHandler)
            ]

            self.assertTrue(len(stream_handlers) > 0)
            self.assertTrue(len(file_handlers) > 0)

            for sh in stream_handlers:
                self.assertIsInstance(
                    sh.formatter, logging_utils.ColoredFormatter
                )

            for fh in file_handlers:
                self.assertNotIsInstance(
                    fh.formatter, logging_utils.ColoredFormatter
                )

    def test_patch_idempotency(self) -> None:
        # Calling patch multiple times should not re-wrap or break
        orig_fn = getattr(mobly_logger, "_setup_test_logger", None)
        mobly_logger_extension._patch_mobly_logger()
        self.assertIs(
            getattr(mobly_logger, "_setup_test_logger", None), orig_fn
        )


if __name__ == "__main__":
    unittest.main()
