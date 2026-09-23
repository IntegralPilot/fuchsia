// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_FAKE_RUNTIME_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_FAKE_RUNTIME_H_

#include <stdint.h>

// This is a fake runtime function that will be exported by the driver host
// and called by the fake root driver.
extern "C" int64_t runtime_get_id();

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_FAKE_RUNTIME_H_
