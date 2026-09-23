// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace {

class DriverServeFidl : public fdf::DriverBase2 {
 public:
  DriverServeFidl() : fdf::DriverBase2("driver_serve_fidl") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("DriverServeFidl started");

    return zx::ok();
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(DriverServeFidl);
