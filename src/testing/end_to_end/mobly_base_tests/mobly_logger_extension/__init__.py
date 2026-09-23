# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly logger extension for Fuchsia tests."""

from __future__ import annotations

import logging
from typing import Any

import logging_utils


def _patch_mobly_logger() -> None:
    """Patches mobly.logger._setup_test_logger to colorize terminal logs."""
    try:
        from mobly import logger as mobly_logger
    except ImportError:
        return

    orig_setup = getattr(mobly_logger, "_setup_test_logger", None)
    if orig_setup and not getattr(orig_setup, "_is_color_patched", False):

        def custom_setup_test_logger(
            log_path: str,
            console_level: int,
            prefix: str | None = None,
            *args: Any,
            **kwargs: Any,
        ) -> None:
            orig_setup(log_path, console_level, prefix, *args, **kwargs)
            logging_utils.colorize_logger(
                logging.getLogger(),
                prefix=prefix,
                format_func=logging_utils.lacewing_log_formatter,
            )

        custom_setup_test_logger._is_color_patched = True  # type: ignore[attr-defined]
        setattr(mobly_logger, "_setup_test_logger", custom_setup_test_logger)


# Patch Mobly logger to colorize terminal output while keeping file logs clean.
_patch_mobly_logger()
