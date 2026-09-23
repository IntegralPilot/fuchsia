// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DRIVER_RUNTIME_BACKED_DISPATCHER_FACTORY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DRIVER_RUNTIME_BACKED_DISPATCHER_FACTORY_H_

#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher-factory.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"

namespace display {

// A DispatcherFactory that creates `DriverRuntimeBackedDispatcher` instances.
class DriverRuntimeBackedDispatcherFactory : public DispatcherFactory {
 public:
  static zx::result<std::unique_ptr<DriverRuntimeBackedDispatcherFactory>> Create();
  DriverRuntimeBackedDispatcherFactory() = default;

  // DispatcherFactory:
  ~DriverRuntimeBackedDispatcherFactory() override = default;

  DriverRuntimeBackedDispatcherFactory(const DriverRuntimeBackedDispatcherFactory&) = delete;
  DriverRuntimeBackedDispatcherFactory(DriverRuntimeBackedDispatcherFactory&&) = delete;
  DriverRuntimeBackedDispatcherFactory& operator=(const DriverRuntimeBackedDispatcherFactory&) =
      delete;
  DriverRuntimeBackedDispatcherFactory& operator=(DriverRuntimeBackedDispatcherFactory&&) = delete;

  // DispatcherFactory:
  zx::result<std::unique_ptr<Dispatcher>> Create(std::string_view name,
                                                 std::string_view scheduler_role) override;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DRIVER_RUNTIME_BACKED_DISPATCHER_FACTORY_H_
