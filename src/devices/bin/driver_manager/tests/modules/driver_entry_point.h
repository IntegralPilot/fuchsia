// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_DRIVER_ENTRY_POINT_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_DRIVER_ENTRY_POINT_H_

#include <stdint.h>

// TODO(https://fxbug.dev): this is just a placeholder until we can sub in
// __fuchsia_driver_registration__.
//
// |data| is an array of additional data for the module.
// |data_len| is the length of |data|.
extern "C" [[gnu::visibility("default")]] int64_t DriverStart(uint64_t* data, int data_len);

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_MODULES_DRIVER_ENTRY_POINT_H_
