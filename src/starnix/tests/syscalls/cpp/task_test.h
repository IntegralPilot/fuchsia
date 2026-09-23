// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_TASK_TEST_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_TASK_TEST_H_

#include <stdint.h>

constexpr uint64_t kCloneVforkSleepUS = 200000;  // 200ms
constexpr uint64_t kCloneVforkSleepNS = kCloneVforkSleepUS * 1000;

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_TASK_TEST_H_
