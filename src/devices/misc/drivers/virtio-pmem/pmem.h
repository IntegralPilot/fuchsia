// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_MISC_DRIVERS_VIRTIO_PMEM_PMEM_H_
#define SRC_DEVICES_MISC_DRIVERS_VIRTIO_PMEM_PMEM_H_

#include <fidl/fuchsia.hardware.virtio.pmem/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/virtio/backends/backend.h>
#include <lib/virtio/device.h>
#include <lib/virtio/ring.h>
#include <stdlib.h>
#include <zircon/compiler.h>

#include <memory>

namespace virtio {

class PmemDevice final : public virtio::Device {
 public:
  PmemDevice(zx::bti bti, std::unique_ptr<Backend> backend, zx::resource mmio_resource);
  ~PmemDevice() final;

  // virtio::Device overrides.
  zx_status_t Init() final;

  void IrqRingUpdate() final;
  void IrqConfigChange() final;
  const char* tag() const final { return "virtio-pmem"; }

  // Produce a clone of the shared buffer VMO suitable for clients to access.
  zx::result<zx::vmo> clone_vmo();

 private:
  // VMO representing the shared buffer.
  zx::vmo phys_vmo_;

  // Virtio event queue. 5.19.2
  Ring request_virtio_queue_;

  zx::resource mmio_resource_;
};

class PmemDriver : public fdf::DriverBase2,
                   public fidl::Server<fuchsia_hardware_virtio_pmem::Device> {
 public:
  static constexpr char kDriverName[] = "virtio-pmem";

  PmemDriver() : fdf::DriverBase2(kDriverName) {}
  ~PmemDriver() override = default;

  // fdf::DriverBase2 implementation.
  zx::result<> Start(fdf::DriverContext context) final;

 private:
  // fidl::Server<fuchsia_hardware_virtiopmem::VirtioPmem> implementation.
  void Get(GetCompleter::Sync& completer) final;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_virtio_pmem::Device>,
                             fidl::UnknownMethodCompleter::Sync& completer) final;

  // Overridden in tests.
  virtual zx::result<std::unique_ptr<PmemDevice>> CreatePmemDevice(
      const std::shared_ptr<fdf::Namespace>& incoming);

  std::unique_ptr<PmemDevice> device_;

  fidl::ServerBindingGroup<fuchsia_hardware_virtio_pmem::Device> bindings_;
};

}  // namespace virtio

#endif  // SRC_DEVICES_MISC_DRIVERS_VIRTIO_PMEM_PMEM_H_
