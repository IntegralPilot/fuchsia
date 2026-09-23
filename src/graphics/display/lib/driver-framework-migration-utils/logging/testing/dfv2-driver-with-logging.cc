// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/testing/dfv2-driver-with-logging.h"

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/zx/result.h>

namespace display::testing {

Dfv2DriverWithLogging::Dfv2DriverWithLogging() : fdf::DriverBase2("dfv2-driver-with-logging") {}
Dfv2DriverWithLogging::~Dfv2DriverWithLogging() = default;

zx::result<> Dfv2DriverWithLogging::Start(fdf::DriverContext context) { return zx::ok(); }

bool Dfv2DriverWithLogging::LogTrace() const { return logging_hardware_module_.LogTrace(); }

bool Dfv2DriverWithLogging::LogDebug() const { return logging_hardware_module_.LogDebug(); }

bool Dfv2DriverWithLogging::LogInfo() const { return logging_hardware_module_.LogInfo(); }

bool Dfv2DriverWithLogging::LogWarning() const { return logging_hardware_module_.LogWarning(); }

bool Dfv2DriverWithLogging::LogError() const { return logging_hardware_module_.LogError(); }

}  // namespace display::testing

FUCHSIA_DRIVER_EXPORT2(::display::testing::Dfv2DriverWithLogging);
