// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "driver_entry_point.h"
#include "v1_driver_entry_point.h"

// This returns the value from calling the v1 driver start function.
int64_t DriverStart(uint64_t* data, int data_len) {
  // We expect the fake driver host to pass one address value, which is the v1 driver's start.
  if (data_len != 1) {
    return 0;
  }
  const auto v1_driver_start_func = reinterpret_cast<decltype(V1DriverStart)*>(data[0]);
  return v1_driver_start_func();
}
