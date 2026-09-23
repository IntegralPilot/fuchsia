// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_TESTING_DFV2_DRIVER_WITH_DISPATCHER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_TESTING_DFV2_DRIVER_WITH_DISPATCHER_H_

#include <lib/async/cpp/irq.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/zx/interrupt.h>

#include <memory>
#include <string>
#include <vector>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/driver-runtime-backed-dispatcher-factory.h"

namespace display {
class DriverRuntimeBackedDispatcher;
}  // namespace display

namespace display::testing {

static constexpr std::string_view kTestDriverSchedulerRole = "fuchsia.test.scheduler.role";
static constexpr std::string_view kTestDriverDispatcherName = "test-dispatcher";

// A DFv2 test driver with a background display::Dispatcher that tests can
// dispatch async tasks or interrupt handlers.
class Dfv2DriverWithDispatcher : public fdf::DriverBase2 {
 public:
  Dfv2DriverWithDispatcher();

  ~Dfv2DriverWithDispatcher() override;

  // Implements `fdf::DriverBase2`.
  zx::result<> Start(fdf::DriverContext context) override;

  // Posts a task on its background dispatcher.
  zx::result<> PostTask(fit::closure task);

  // Starts an IRQ handler on its background dispatcher.
  zx::result<> StartIrqHandler(zx::interrupt irq, async::Irq::Handler handler);

  // Shutdown the background dispatcher.
  void ShutdownDispatcher();

 private:
  struct IrqAndHandler {
    zx::interrupt irq;
    std::unique_ptr<async::Irq> handler;
  };

  // Downcasts the abstract `dispatcher_` to its concrete type.
  //
  // This is safe because `dispatcher_` is initialized in `Start()` via
  // `dispatcher_factory_.Create()`, which uses `DriverRuntimeBackedDispatcher::Create()`.
  // The concrete type of the dispatcher returned by this factory is guaranteed
  // to be `DriverRuntimeBackedDispatcher`.
  display::DriverRuntimeBackedDispatcher* driver_runtime_backed_dispatcher() const;

  DriverRuntimeBackedDispatcherFactory dispatcher_factory_;
  std::unique_ptr<Dispatcher> dispatcher_;
  std::vector<IrqAndHandler> irq_and_handlers_;
};

}  // namespace display::testing

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_TESTING_DFV2_DRIVER_WITH_DISPATCHER_H_
