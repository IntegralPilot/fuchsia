// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/virtio/driver_utils.h>

#include "socket.h"

namespace virtio {

class SocketDriver : public fdf::DriverBase2 {
 public:
  static constexpr char kDriverName[] = "virtio-socket";

  SocketDriver() : fdf::DriverBase2(kDriverName) {}
  ~SocketDriver() override = default;

  zx::result<> Start(fdf::DriverContext context) final;

 private:
  std::unique_ptr<SocketDevice> device_;
};

zx::result<> SocketDriver::Start(fdf::DriverContext context) {
  zx::result pci_client_result =
      context.incoming().Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_result.is_error()) {
    fdf::error("Failed to get pci client: {}", pci_client_result);
    return pci_client_result.take_error();
  }

  zx::result bti_and_backend_result =
      virtio::GetBtiAndBackend(std::move(pci_client_result).value());
  if (!bti_and_backend_result.is_ok()) {
    fdf::error("GetBtiAndBackend failed: {}", bti_and_backend_result);
    return bti_and_backend_result.take_error();
  }
  auto [bti, backend] = std::move(bti_and_backend_result).value();

  device_ = std::make_unique<SocketDevice>(std::move(bti), std::move(backend));

  zx_status_t status = device_->Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // Export the service
  zx::result<> result = outgoing()->AddService<fuchsia_hardware_vsock::Service>(
      fuchsia_hardware_vsock::Service::InstanceHandler({
          .device = device_->GetHandler(),
      }));
  if (result.is_error()) {
    fdf::error("Failed to add service: {}", result.status_string());
    return result.take_error();
  }

  return zx::ok();
}

}  // namespace virtio

FUCHSIA_DRIVER_EXPORT2(virtio::SocketDriver);
