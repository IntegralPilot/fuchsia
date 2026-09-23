# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

load("@fuchsia_build_config//:defs.bzl", "build_config")

# Use `target_compatible_with = HOST_OS_CONSTRAINTS` to specify
# that a target definition should only be built for the host
# operating system, independent of its CPU architecture.
#
# This is useful for IDK host tools that need to be cross-compiled
# for both linux/x64 and linux/arm64.
HOST_OS_CONSTRAINTS = [build_config.host_platform_os_constraint]
