// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/misc/drivers/virtio-pmem/pmem.h"

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/virtio/driver_utils.h>
#include <limits.h>
#include <zircon/status.h>

#include <memory>
#include <utility>

#include <fbl/auto_lock.h>

#include "src/devices/misc/drivers/virtio-pmem/virtio/pmem.h"

namespace virtio {

PmemDevice::PmemDevice(zx::bti bti, std::unique_ptr<Backend> backend, zx::resource mmio_resource)
    : virtio::Device(std::move(bti), std::move(backend)),
      request_virtio_queue_(this),
      mmio_resource_(std::move(mmio_resource)) {}

PmemDevice::~PmemDevice() {}

zx_status_t PmemDevice::Init() {
  fdf::debug("initialization starting");
  // reset the device
  DeviceReset();

  // ack and set the driver status bit
  DriverStatusAck();

  // Note: We don't support VIRTIO_PMEM_F_SHMEM_REGION
  if (DeviceFeaturesSupported() & VIRTIO_F_VERSION_1) {
    DriverFeaturesAck(VIRTIO_F_VERSION_1);
    if (zx_status_t status = DeviceStatusFeaturesOk(); status != ZX_OK) {
      fdf::error("Feature negotiation failed: {}", zx_status_get_string(status));
      return status;
    }
  }

  // Read device configuration space.
  virtio_pmem_config config{};
  ReadDeviceConfig(offsetof(virtio_pmem_config, start), &config.start);
  ReadDeviceConfig(offsetof(virtio_pmem_config, size), &config.size);
  fdf::debug("config address: {:#x} length {:#x}", config.start, config.size);

  const size_t rounded_size = fbl::round_up<size_t>(config.size, zx_system_get_page_size());

  zx_status_t status =
      zx::vmo::create_physical(mmio_resource_, config.start, rounded_size, &phys_vmo_);
  if (status != ZX_OK) {
    fdf::error("failed to create VMO: {}", zx_status_get_string(status));
    return status;
  }
  // Physical VMOs have a default cache policy of uncached. The persistent
  // memory object exposes a region of normal memory (not device memory) so a
  // cached policy is more appropriate.
  status = phys_vmo_.set_cache_policy(ZX_CACHE_POLICY_CACHED);
  if (status != ZX_OK) {
    fdf::error("failed to set cache policy: {}", zx_status_get_string(status));
    return status;
  }

  // Initialize request virtqueue.
  status = request_virtio_queue_.Init(0);
  if (status != ZX_OK) {
    fdf::error("failed to initialize req_vq : {}", zx_status_get_string(status));
    return status;
  }

  // set DRIVER_OK
  DriverStatusOk();

  fdf::debug("initialization succeeded");

  return ZX_OK;
}

void PmemDevice::IrqRingUpdate() { fdf::debug("{}: Got irq ring update, ignoring", tag()); }

void PmemDevice::IrqConfigChange() { fdf::debug("{}: Got irq config change, ignoring", tag()); }

zx::result<zx::vmo> PmemDevice::clone_vmo() {
  zx::vmo vmo;
  zx_status_t status = phys_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(vmo));
}

zx::result<> PmemDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  zx::result device = CreatePmemDevice(incoming);

  if (device.is_error()) {
    return device.take_error();
  }

  device_ = std::move(*device);

  zx_status_t status = device_->Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  // Advertise service.
  fuchsia_hardware_virtio_pmem::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                        fidl::kIgnoreBindingClosure),
  });
  zx::result add_result =
      outgoing()->AddService<fuchsia_hardware_virtio_pmem::Service>(std::move(handler));
  if (add_result.is_error()) {
    fdf::error("Unable to add service: {}", add_result);
    return add_result.take_error();
  }

  return zx::ok();
}

void PmemDriver::Get(GetCompleter::Sync& completer) {
  if (device_) {
    zx::result cloned_vmo = device_->clone_vmo();
    completer.Reply({std::move(cloned_vmo)});
  } else {
    fdf::warn("Get called with uninitialized device.");
    completer.Close(ZX_ERR_BAD_STATE);
  }
}

void PmemDriver::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_virtio_pmem::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("Unknown FIDL method received ordinal {}, closing channel", metadata.method_ordinal);
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

zx::result<std::unique_ptr<PmemDevice>> PmemDriver::CreatePmemDevice(
    const std::shared_ptr<fdf::Namespace>& incoming) {
  zx::result pci_client_result = incoming->Connect<fuchsia_hardware_pci::Service::Device>();
  if (pci_client_result.is_error()) {
    fdf::error("Failed to get pci client: {}", pci_client_result);
    return pci_client_result.take_error();
  }

  zx::result mmio_result = incoming->Connect<fuchsia_kernel::MmioResource>();
  if (mmio_result.is_error()) {
    fdf::error("Failed to connect to MmioResource: {}", mmio_result);
    return mmio_result.take_error();
  }
  fidl::WireResult mmio_resource = fidl::WireCall(*mmio_result)->Get();
  if (!mmio_resource.ok()) {
    fdf::error("Failed to get mmio resource: {}", mmio_resource.status_string());
    return zx::error(mmio_resource.status());
  }

  zx::result bti_and_backend_result =
      virtio::GetBtiAndBackend(std::move(pci_client_result).value());
  if (!bti_and_backend_result.is_ok()) {
    fdf::error("GetBtiAndBackend failed: {}", bti_and_backend_result);
    return bti_and_backend_result.take_error();
  }
  auto [bti, backend] = std::move(bti_and_backend_result).value();

  return zx::ok(std::make_unique<PmemDevice>(std::move(bti), std::move(backend),
                                             std::move(mmio_resource->resource)));
}

}  // namespace virtio
