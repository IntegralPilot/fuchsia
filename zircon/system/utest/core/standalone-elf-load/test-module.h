// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_STANDALONE_ELF_LOAD_TEST_MODULE_H_
#define ZIRCON_SYSTEM_UTEST_CORE_STANDALONE_ELF_LOAD_TEST_MODULE_H_

#include <cstdint>

// This is the entry point in the test module.
using TestStartFunction = int(int x, int y);
extern "C" TestStartFunction TestStart;

// This is the value at the "kTestRoData" symbol.
constexpr uint32_t kTestRoDataValue = 42;

#endif  // ZIRCON_SYSTEM_UTEST_CORE_STANDALONE_ELF_LOAD_TEST_MODULE_H_
