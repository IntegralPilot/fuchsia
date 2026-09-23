#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

# Compile and recreate the static library libtest_prebuilt.a.
# This uses clang and ar from the host environment.
clang -c libtest_prebuilt.c -o libtest_prebuilt.o
ar rcs libtest_prebuilt.a libtest_prebuilt.o
rm libtest_prebuilt.o

echo "Successfully regenerated libtest_prebuilt.a!"
