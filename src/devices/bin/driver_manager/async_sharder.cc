// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/async_sharder.h"

namespace driver_manager {

AsyncSharder::AsyncSharder(size_t count, fit::callback<void(zx::result<>)> complete_callback)
    : remaining_(count), complete_callback_(std::move(complete_callback)) {}

AsyncSharder::~AsyncSharder() { ZX_ASSERT_MSG(remaining_ == 0, "Sharder not complete"); }

void AsyncSharder::CompleteShard() {
  if (--remaining_ == 0 && complete_callback_) {
    complete_callback_(zx::ok());
  }
}

void AsyncSharder::CompleteShardError(zx_status_t status) {
  remaining_--;
  if (complete_callback_) {
    complete_callback_(zx::error(status));
  }
}

}  // namespace driver_manager
