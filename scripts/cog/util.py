# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utility functions for cog scripts."""

import re


def sanitize_filename(filename: str) -> str:
    """Sanitizes a string to be used as a safe filename.

    Replaces characters that are not alphanumeric, underscores, periods, or
    hyphens with hyphens and converts the string to lowercase.
    """
    return re.sub(r"[^a-zA-Z0-9._-]+", "-", filename).lower()
