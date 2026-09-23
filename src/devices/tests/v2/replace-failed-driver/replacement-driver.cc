// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>

namespace {

class FailerReplacementDriver : public fdf::DriverBase2 {
 public:
  FailerReplacementDriver() : fdf::DriverBase2("failer-replacement") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("This driver is returning OK.");
    return zx::ok();
  }
};

}  // namespace

FUCHSIA_DRIVER_EXPORT2(FailerReplacementDriver);
