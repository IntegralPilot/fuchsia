// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/driver-runtime-backed-dispatcher-factory.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <memory>
#include <string_view>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/driver-runtime-backed-dispatcher.h"

namespace display {

// static
zx::result<std::unique_ptr<DriverRuntimeBackedDispatcherFactory>>
DriverRuntimeBackedDispatcherFactory::Create() {
  fbl::AllocChecker alloc_checker;
  auto factory = fbl::make_unique_checked<DriverRuntimeBackedDispatcherFactory>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DriverRuntimeBackedDispatcherFactory");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(factory));
}

zx::result<std::unique_ptr<Dispatcher>> DriverRuntimeBackedDispatcherFactory::Create(
    std::string_view name, std::string_view scheduler_role) {
  return DriverRuntimeBackedDispatcher::Create(name, scheduler_role);
}

}  // namespace display
