// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples.gizmo/cpp/wire.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/device.h>

#include <ddktl/device.h>

namespace zircon_transport {

class ZirconTransportDevice;
using DeviceType = ddk::Device<ZirconTransportDevice, ddk::Initializable>;

class ZirconTransportDevice : public DeviceType,
                              public fidl::WireServer<fuchsia_examples_gizmo::Device> {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  explicit ZirconTransportDevice(zx_device_t* parent, async_dispatcher_t* dispatcher)
      : DeviceType(parent), outgoing_(dispatcher), dispatcher_(dispatcher) {}
  virtual ~ZirconTransportDevice() = default;

  zx_status_t Bind();

  void GetHardwareId(GetHardwareIdCompleter::Sync& completer) override;
  void GetFirmwareVersion(GetFirmwareVersionCompleter::Sync& completer) override;

  // Device protocol implementation.
  void DdkInit(ddk::InitTxn txn) { txn.Reply(ZX_OK); }
  void DdkRelease() { delete this; }

 private:
  component::OutgoingDirectory outgoing_;
  async_dispatcher_t* dispatcher_;
};

// static
zx_status_t ZirconTransportDevice::Create(void* ctx, zx_device_t* parent) {
  async_dispatcher_t* dispatcher =
      fdf_dispatcher_get_async_dispatcher(fdf_dispatcher_get_current_dispatcher());
  auto device = std::make_unique<zircon_transport::ZirconTransportDevice>(parent, dispatcher);

  zx_status_t status = device->Bind();
  if (status == ZX_OK) {
    // Driver framework now owns device.
    [[maybe_unused]] auto* dev = device.release();
  }
  return status;
}

zx_status_t ZirconTransportDevice::Bind() {
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
                    .set_flags(DEVICE_ADD_MUST_ISOLATE)
                    .set_fidl_service_offers(offers)
                    .set_outgoing_dir(client_end.TakeChannel()));
}

void ZirconTransportDevice::GetHardwareId(GetHardwareIdCompleter::Sync& completer) {
  completer.ReplySuccess(0x1234ABCD);
}

void ZirconTransportDevice::GetFirmwareVersion(GetFirmwareVersionCompleter::Sync& completer) {
  completer.ReplySuccess(0x0, 0x1);
}

}  // namespace zircon_transport

static zx_driver_ops_t zircon_transport_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = zircon_transport::ZirconTransportDevice::Create;
  return ops;
}();

ZIRCON_DRIVER(ZirconTransportDevice, zircon_transport_driver_ops, "zircon", "0.1");
