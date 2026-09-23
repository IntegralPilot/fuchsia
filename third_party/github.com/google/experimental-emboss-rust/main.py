#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Entrypoint wrapper for embossc."""

import sys
import embossc_lib


def main() -> int:
    return embossc_lib.main(sys.argv)
