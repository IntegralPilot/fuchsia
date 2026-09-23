// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_LOOP_BACKED_DISPATCHER_FACTORY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_LOOP_BACKED_DISPATCHER_FACTORY_H_

#include <lib/ddk/device.h>
#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include <fbl/alloc_checker.h>

#include "lib/async-loop/loop.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher-factory.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"

namespace display {

// A DispatcherFactory that creates `LoopBackedDispatcher` instances.
class LoopBackedDispatcherFactory : public DispatcherFactory {
 public:
  static zx::result<std::unique_ptr<LoopBackedDispatcherFactory>> Create(zx_device_t* device);
  explicit LoopBackedDispatcherFactory(zx_device_t* device);

  // DispatcherFactory:
  ~LoopBackedDispatcherFactory() override;

  LoopBackedDispatcherFactory(const LoopBackedDispatcherFactory&) = delete;
  LoopBackedDispatcherFactory(LoopBackedDispatcherFactory&&) = delete;
  LoopBackedDispatcherFactory& operator=(const LoopBackedDispatcherFactory&) = delete;
  LoopBackedDispatcherFactory& operator=(LoopBackedDispatcherFactory&&) = delete;

  // DispatcherFactory:
  zx::result<std::unique_ptr<Dispatcher>> Create(std::string_view name,
                                                 std::string_view scheduler_role) override;

 private:
  zx_device_t* const device_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_LOOP_BACKED_DISPATCHER_FACTORY_H_
