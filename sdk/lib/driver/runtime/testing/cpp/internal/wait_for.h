// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_WAIT_FOR_H_
#define LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_WAIT_FOR_H_

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>

namespace fdf_internal {

// If no unmanaged driver dispatchers exist, returns immediately.
// Otherwise waits for the condition to become true while running the unmanaged
// driver dispatchers with |fdf_testing_run_until_idle|.
//
// Returns an OK result when either of the following happen:
//
// - The condition is true.
// - All driver dispatchers are managed by the driver runtime thread-pool.
//
// Propagates any other error from running the unmanaged dispatchers.
//
// |condition| must be a non-blocking function.
//
// This MUST be called from the main test thread.
zx::result<> IfExistsRunUnmanagedUntil(fit::function<bool()> condition);

}  // namespace fdf_internal

#endif  // LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_WAIT_FOR_H_
