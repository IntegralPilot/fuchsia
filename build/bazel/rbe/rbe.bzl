# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This is used to tell a target that it needs to use the larger RBE workers, which have more memory
# and more cores available.
RBE_USE_LARGE_WORKER = {"Pool": "large_worker"}
