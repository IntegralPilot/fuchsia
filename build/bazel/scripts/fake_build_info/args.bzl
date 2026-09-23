# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This file exists to enable running minimalist bazel
# build and test commands without requiring Fuchsia-specific
# configuration.

# Do not depend on a real value.
target_cpu = "null"
