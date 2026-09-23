# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
shac.star defines static analysis checks to run in infrastructure and local
workflows using the SHAC tool.

See https://fuchsia.googlesource.com/shac-project/shac for more information.
"""

# Checks are registered in a more deeply nested file so that the root shac.star
# file need not be modified frequently.
load("//scripts/shac/main.star", "register_all_checks")

register_all_checks()
