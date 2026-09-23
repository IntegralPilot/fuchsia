// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_TEST_SANITIZER_DEBUGDATA_H_
#define LIB_C_TEST_SANITIZER_DEBUGDATA_H_

#include <cstdint>

inline constexpr char kTestName[] = "test";
inline constexpr uint8_t kTestData[] = {0x00, 0x11, 0x22, 0x33};

#endif  // LIB_C_TEST_SANITIZER_DEBUGDATA_H_
