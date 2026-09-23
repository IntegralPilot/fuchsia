// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_TESTS_END_TO_END_POWER_POWER_UTILS_H_
#define SRC_TESTS_END_TO_END_POWER_POWER_UTILS_H_

#include <cstdint>

namespace power {
void intenseComputation(uint64_t duration);
void intenseComputationOnAllCores(uint64_t duration);
void idleThread(uint64_t duration);
}  // namespace power

#endif  // SRC_TESTS_END_TO_END_POWER_POWER_UTILS_H_
