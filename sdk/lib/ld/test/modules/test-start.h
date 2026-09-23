// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_MODULES_TEST_START_H_
#define LIB_LD_TEST_MODULES_TEST_START_H_

#include <stdint.h>

extern "C" [[gnu::visibility("default")]] int64_t TestStart();

#endif  // LIB_LD_TEST_MODULES_TEST_START_H_
