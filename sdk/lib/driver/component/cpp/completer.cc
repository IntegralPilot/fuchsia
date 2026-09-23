// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <zircon/assert.h>

namespace fdf {

Completer::~Completer() {
  ZX_ASSERT_MSG(callback_ == std::nullopt, "Completer was not called before going out of scope.");
}

void Completer::operator()(zx::result<> result) {
  ZX_ASSERT_MSG(callback_ != std::nullopt, "Cannot call Completer more than once.");
  auto callback = std::move(callback_.value());
  callback_.reset();
  callback(result);
}

}  // namespace fdf
