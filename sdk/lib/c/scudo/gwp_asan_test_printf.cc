// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gwp_asan/optional/printf.h"
#include "src/string_utils.h"

namespace gwp_asan::test {
// Use scudo-defined Printf() in gwp-asan unittests.
Printf_t getPrintfFunction() { return scudo::Printf; }
}  // namespace gwp_asan::test
