// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "deps.h"
#include "driver_entry_point.h"
#include "fake_runtime.h"

// This calls the fake runtime function, as well as the implementation of a()
// defined in fake_root_driver_deps.cc
//
// runtime_get_id() -> 14
// In this driver's domain a() -> 10.
int64_t DriverStart(uint64_t* data, int data_len) { return runtime_get_id() + a(); }
