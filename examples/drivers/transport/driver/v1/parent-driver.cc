// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples.gizmo/cpp/driver/wire.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/device.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>

#include <ddktl/device.h>

namespace driver_transport {

class DriverTransportDevice;
using DeviceType = ddk::Device<DriverTransportDevice, ddk::Initializable>;

class DriverTransportDevice : public DeviceType,
                              public fdf::WireServer<fuchsia_examples_gizmo::Device> {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  explicit DriverTransportDevice(zx_device_t* parent, fdf_dispatcher_t* dispatcher)
      : DeviceType(parent),
        outgoing_(fdf::OutgoingDirectory::Create(dispatcher)),
        dispatcher_(dispatcher) {}
  virtual ~DriverTransportDevice() = default;

  zx_status_t Bind();

  void GetHardwareId(fdf::Arena& arena, GetHardwareIdCompleter::Sync& completer) override;
  void GetFirmwareVersion(fdf::Arena& arena, GetFirmwareVersionCompleter::Sync& completer) override;

  // Device protocol implementation.
  void DdkInit(ddk::InitTxn txn) { txn.Reply(ZX_OK); }
  void DdkRelease() { delete this; }

 private:
  fdf::OutgoingDirectory outgoing_;
  fdf_dispatcher_t* dispatcher_;
};

// static
zx_status_t DriverTransportDevice::Create(void* ctx, zx_device_t* parent) {
  fdf_dispatcher_t* dispatcher = fdf_dispatcher_get_current_dispatcher();
  auto device = std::make_unique<driver_transport::DriverTransportDevice>(parent, dispatcher);

  zx_status_t status = device->Bind();
  if (status == ZX_OK) {
    // Driver framework now owns device.
    [[maybe_unused]] auto* dev = device.release();
  }
  return status;
}

zx_status_t DriverTransportDevice::Bind() {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  // Publish `fuchsia.examples.gizmo.Service` to the outgoing directory.
  fuchsia_examples_gizmo::Service::InstanceHandler handler({.device = bind_handler(dispatcher_)});
  auto status = outgoing_.AddService<fuchsia_examples_gizmo::Service>(std::move(handler));
  if (status.is_error()) {
    return status.status_value();
  }

  status = outgoing_.Serve(std::move(server_end));
  if (status.is_error()) {
    return status.status_value();
  }

  std::array offers = {
      fuchsia_examples_gizmo::Service::Name,
  };
  return DdkAdd(ddk::DeviceAddArgs("transport-child")
                    .set_runtime_service_offers(offers)
                    .set_outgoing_dir(client_end.TakeChannel()));
}

void DriverTransportDevice::GetHardwareId(fdf::Arena& arena,
                                          GetHardwareIdCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(0x1234ABCD);
}

void DriverTransportDevice::GetFirmwareVersion(fdf::Arena& arena,
                                               GetFirmwareVersionCompleter::Sync& completer) {
  completer.buffer(arena).ReplySuccess(0x0, 0x1);
}

}  // namespace driver_transport

static zx_driver_ops_t driver_transport_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = driver_transport::DriverTransportDevice::Create;
  return ops;
}();

ZIRCON_DRIVER(DriverTransportDevice, driver_transport_driver_ops, "zircon", "0.1");
