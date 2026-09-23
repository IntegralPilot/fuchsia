// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.interrupt/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/time.h>

namespace interrupt_controller {

class TestInterruptControllerDriver
    : public fdf::DriverBase2,
      public fidl::WireServer<fuchsia_hardware_interrupt::Controller> {
 public:
  static constexpr std::string_view kDriverName = "test-interrupt-controller";

  TestInterruptControllerDriver() : fdf::DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

  // fuchsia.hardware.interrupt.Controller implementation.
  void RegisterInterrupt(RegisterInterruptRequestView request,
                         RegisterInterruptCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_interrupt::Controller> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_interrupt::Controller>> binding_;
};

zx::result<> TestInterruptControllerDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  zx::result registry_client =
      incoming->Connect<fuchsia_hardware_interrupt::ControllerRegistryService::Registry>();
  if (registry_client.is_error()) {
    fdf::error("Failed to connect to ControllerRegistryService: {}",
               registry_client.status_string());
    return registry_client.take_error();
  }

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_hardware_interrupt::Controller>::Create();

  binding_.emplace(dispatcher(), std::move(server_end), this, fidl::kIgnoreBindingClosure);

  auto result = fidl::WireCall(*registry_client)->RegisterController(std::move(client_end));
  if (!result.ok()) {
    fdf::error("Call to RegisterController failed: {}", result.error().FormatDescription());
    return zx::error(result.error().status());
  }
  if (result->is_error()) {
    fdf::error("RegisterController failed: {}", zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }

  std::vector<fuchsia_driver_framework::Offer> offers{
      fdf::MakeOffer2<fuchsia_hardware_interrupt::ControllerRegistryService>(),
  };

  zx::result child =
      AddChild(kDriverName, std::vector<fuchsia_driver_framework::NodeProperty2>{}, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child.status_string());
    return child.take_error();
  }

  child_ = std::move(child.value());
  return zx::ok();
}

void TestInterruptControllerDriver::RegisterInterrupt(RegisterInterruptRequestView request,
                                                      RegisterInterruptCompleter::Sync& completer) {
  // Hardcoded in the board driver.
  if (request->irq != 42) {
    completer.Reply(zx::error(ZX_ERR_NOT_FOUND));
    return;
  }
  if (request->mode != fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh) {
    fdf::error("Unexpected mode {}", static_cast<uint32_t>(request->mode));
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  zx_status_t status = request->interrupt.trigger(0, zx::time_boot());
  if (status != ZX_OK) {
    completer.Reply(zx::error(status));
  } else {
    completer.Reply(zx::ok());
  }
}

}  // namespace interrupt_controller

FUCHSIA_DRIVER_EXPORT2(interrupt_controller::TestInterruptControllerDriver);
