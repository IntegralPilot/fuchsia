// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/testing/dfv1-driver-with-logging.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/driver.h>
#include <zircon/errors.h>

#include <fbl/alloc_checker.h>

namespace display::testing {

// static
zx_status_t Dfv1DriverWithLogging::Create(void* ctx, zx_device_t* parent) {
  fbl::AllocChecker alloc_checker;
  auto device = fbl::make_unique_checked<Dfv1DriverWithLogging>(&alloc_checker, parent);
  if (!alloc_checker.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  zx_status_t bind_status = device->Bind();
  if (bind_status != ZX_OK) {
    return bind_status;
  }

  // driver manager is now in charge of the device.
  [[maybe_unused]] Dfv1DriverWithLogging* released_device = device.release();
  return ZX_OK;
}

Dfv1DriverWithLogging::Dfv1DriverWithLogging(zx_device_t* parent) : DeviceType(parent) {}

Dfv1DriverWithLogging::~Dfv1DriverWithLogging() = default;

void Dfv1DriverWithLogging::DdkRelease() { delete this; }

bool Dfv1DriverWithLogging::LogTrace() const { return logging_hardware_module_.LogTrace(); }

bool Dfv1DriverWithLogging::LogDebug() const { return logging_hardware_module_.LogDebug(); }

bool Dfv1DriverWithLogging::LogInfo() const { return logging_hardware_module_.LogInfo(); }

bool Dfv1DriverWithLogging::LogWarning() const { return logging_hardware_module_.LogWarning(); }

bool Dfv1DriverWithLogging::LogError() const { return logging_hardware_module_.LogError(); }

zx_status_t Dfv1DriverWithLogging::Bind() { return DdkAdd("dfv1-device-with-logging"); }

static constexpr zx_driver_ops_t kDriverOps = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Dfv1DriverWithLogging::Create;
  return ops;
}();

}  // namespace display::testing

ZIRCON_DRIVER(dfv1_driver_with_logging, ::display::testing::kDriverOps, "zircon", "0.1");
