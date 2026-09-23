#!/usr/bin/env fuchsia-vendored-python

# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import sys

import lib


def main() -> int:
    lib.f()
    return 0


if __name__ == "__main__":
    sys.exit(main())
