// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTEL_DISPLAY_DRIVER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTEL_DISPLAY_DRIVER_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/zx/result.h>

#include <memory>

#include "src/graphics/display/drivers/intel-display/intel-display.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"

namespace intel_display {

// Driver instance that binds to the intel-display PCI device.
//
// This class is responsible for interfacing with the Fuchsia Driver Framework.
class IntelDisplayDriver : public fdf::DriverBase2 {
 public:
  explicit IntelDisplayDriver();
  ~IntelDisplayDriver() override;

  // fdf::DriverBase:
  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;
  void Stop(fdf::StopCompleter completer) override;

  zx::result<ddk::AnyProtocol> GetProtocol(uint32_t proto_id);

  Controller* controller() const { return controller_.get(); }

 private:
  zx::result<> InitController();

  // Must be called after `InitController()`.
  zx::result<> InitDisplayNode();

  // Must be called after `InitController()`.
  zx::result<> InitGpuCoreNode(const std::optional<std::string>& node_name);

  void PrepareStopOnPowerOn(fdf::StopCompleter completer);
  void PrepareStopOnPowerStateTransition(fuchsia_system_state::SystemPowerState power_state,
                                         fdf::StopCompleter completer);

  // Must outlive `controller_` and `engine_fidl_adapter_`.
  display::DisplayEngineEventsFidl engine_events_;

  // Must outlive `engine_fidl_adapter_`.
  std::unique_ptr<Controller> controller_;

  std::unique_ptr<display::DisplayEngineFidlAdapter> engine_fidl_adapter_;

  std::optional<zbi_swfb_t> framebuffer_info_;
  zx::resource mmio_resource_;
  zx::resource ioport_resource_;

  std::optional<compat::BanjoServer> gpu_banjo_server_;
  compat::SyncInitializedDeviceServer gpu_compat_server_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> display_node_controller_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> gpu_core_node_controller_;
  std::shared_ptr<fdf::Namespace> incoming_;
  inspect::Inspector inspector_;
  std::optional<inspect::ComponentInspector> component_inspector_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_INTEL_DISPLAY_DRIVER_H_
