// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>

namespace {

class FailerDriver : public fdf::DriverBase2 {
 public:
  FailerDriver() : fdf::DriverBase2("failer") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("This driver is returning a ZX_ERR_NOT_SUPPORTED error.");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(FailerDriver);
