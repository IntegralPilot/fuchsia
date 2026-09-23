// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_V1_DRIVER_ENTRY_POINT_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_V1_DRIVER_ENTRY_POINT_H_

#include <stdint.h>

// This is the entry point to the fake v1 driver.
extern "C" [[gnu::visibility("default")]] int64_t V1DriverStart();

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_V1_DRIVER_ENTRY_POINT_H_
