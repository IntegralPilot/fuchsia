# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Exception utilities for Mobly tests."""

import logging
from typing import NoReturn

_LOGGER = logging.getLogger(__name__)


def unroll_and_raise(exc: BaseException) -> NoReturn:
    """Logs the full ancestry tree and raises the earliest root cause found via context."""
    seen = set()
    log_entries = []

    def _traverse(
        e: BaseException, level: int = 0, is_main_path: bool = True
    ) -> BaseException:
        if id(e) in seen:
            return e
        seen.add(id(e))

        log_entries.append((e, level, is_main_path))

        ctx = getattr(e, "__context__", None)
        cause = getattr(e, "__cause__", None)

        # Strategy: Prioritize context as the chronological 'main' ancestor.
        main_ancestor = ctx or cause

        # If both exist, follow the cause branch as a 'side' traversal.
        side_ancestor = cause if (ctx and cause) else None

        if side_ancestor:
            _traverse(side_ancestor, level + 1, is_main_path=False)

        if main_ancestor:
            return _traverse(
                main_ancestor, level + 1, is_main_path=is_main_path
            )

        return e

    root = _traverse(exc, level=0, is_main_path=True)

    if len(log_entries) > 1:
        _LOGGER.warning(
            "Multiple exceptions occurred during execution and teardown:"
        )
        for i, (e, level, is_main) in enumerate(log_entries):
            prefix = "  " * level
            tag = "[Primary]" if is_main else "[Explanation]"
            _LOGGER.warning(
                f"{prefix}{tag} Exception {len(log_entries)-i}: {type(e).__name__}: {e}"
            )

    raise root
