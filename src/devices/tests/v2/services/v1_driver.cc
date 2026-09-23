// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.services.test/cpp/wire.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/inspect/cpp/inspect.h>

#include <ddktl/device.h>
#include <ddktl/fidl.h>
#include <ddktl/protocol/empty-protocol.h>

namespace add_service {

class AddServiceDevice;
using DeviceType = ddk::Device<AddServiceDevice, ddk::Initializable>;
class AddServiceDevice : public DeviceType,
                         public fidl::WireServer<fuchsia_services_test::ControlPlane>,
                         public fidl::WireServer<fuchsia_services_test::DataPlane> {
 public:
  explicit AddServiceDevice(zx_device_t* root) : DeviceType(root) {}
  virtual ~AddServiceDevice() = default;

  static zx_status_t Bind(void* ctx, zx_device_t* dev) {
    auto driver = std::make_unique<AddServiceDevice>(dev);
    zx_status_t result =
        driver->DdkAdd(ddk::DeviceAddArgs("root").set_flags(DEVICE_ADD_NON_BINDABLE));
    if (result != ZX_OK) {
      return result;
    }
    result = driver->AddServices();
    if (result != ZX_OK) {
      return result;
    }

    // The driver framework now owns driver.
    [[maybe_unused]] auto ptr = driver.release();
    return ZX_OK;
  }

  zx_status_t AddServices() {
    // Advertise a service:
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    fuchsia_services_test::Device::InstanceHandler handler({
        .control = control_bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
        .data = data_bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure),
    });

    zx::result add_result = DdkAddService<fuchsia_services_test::Device>(std::move(handler));
    if (add_result.is_error()) {
      zxlogf(ERROR, "Failed to add service");
      return add_result.status_value();
    }
    return ZX_OK;
  }

  void DdkInit(ddk::InitTxn txn) { txn.Reply(ZX_OK); }

  void DdkRelease() { delete this; }

 private:
  // fidl::WireServer<ft::ControlPlane>
  void ControlDo(ControlDoCompleter::Sync& completer) override { completer.Reply(); }

  // fidl::WireServer<ft::DataPlane>
  void DataDo(DataDoCompleter::Sync& completer) override { completer.Reply(); }
  fidl::ServerBindingGroup<fuchsia_services_test::ControlPlane> control_bindings_;
  fidl::ServerBindingGroup<fuchsia_services_test::DataPlane> data_bindings_;
};

static zx_driver_ops_t root_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = AddServiceDevice::Bind;
  return ops;
}();

}  // namespace add_service

ZIRCON_DRIVER(AddService, add_service::root_driver_ops, "zircon", "0.1");
