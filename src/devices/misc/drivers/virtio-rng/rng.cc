// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rng.h"

#include <inttypes.h>
#include <lib/virtio/driver_utils.h>
#include <limits.h>
#include <zircon/status.h>

#include <memory>
#include <utility>

#include <fbl/auto_lock.h>

#define LOCAL_TRACE 0

namespace virtio {

RngDevice::RngDevice(zx::bti bti, std::unique_ptr<Backend> backend)
    : virtio::Device(std::move(bti), std::move(backend)) {}

RngDevice::~RngDevice() {
  // TODO: clean up allocated physical memory
}

zx_status_t RngDevice::Init() {
  // reset the device
  DeviceReset();

  // ack and set the driver status bit
  DriverStatusAck();

  if (DeviceFeaturesSupported() & VIRTIO_F_VERSION_1) {
    DriverFeaturesAck(VIRTIO_F_VERSION_1);
    if (zx_status_t status = DeviceStatusFeaturesOk(); status != ZX_OK) {
      fdf::error("Feature negotiation failed: {}", zx_status_get_string(status));
      return status;
    }
  }

  // allocate the main vring
  zx_status_t status = vring_.Init(kRingIndex, kRingSize);
  if (status != ZX_OK) {
    fdf::error("{}: failed to allocate vring", tag());
    return status;
  }

  // allocate the entropy buffer
  assert(kBufferSize <= zx_system_get_page_size());
  auto factory = dma_buffer::CreateBufferFactory();
  status = factory->CreateContiguous(bti(), zx_system_get_page_size(), 0,
                                     dma_buffer::CacheOptions::kEnabled, &buf_);
  if (status != ZX_OK) {
    fdf::error("{}: cannot allocate entropy buffer: {}", tag(), status);
    return status;
  }

  fdf::debug("{}: allocated entropy buffer at {:p}, physical address {:#x}", tag(), buf_->virt(),
             buf_->phys());

  // start the interrupt thread
  StartIrqThread();

  // set DRIVER_OK
  DriverStatusOk();

  // TODO(https://fxbug.dev/42098992): The kernel should trigger entropy requests, instead of
  // relying on this userspace thread to push entropy whenever it wants to. As a temporary hack,
  // this thread pushes entropy to the kernel every 300 seconds instead.
  thrd_create_with_name(&seed_thread_, RngDevice::SeedThreadEntry, this, "virtio-rng-seed-thread");
  thrd_detach(seed_thread_);

  fdf::info("{}: initialization succeeded", tag());

  return ZX_OK;
}

void RngDevice::IrqRingUpdate() {
  fdf::debug("{}: Got irq ring update", tag());

  // parse our descriptor chain, add back to the free queue
  auto free_chain = [this](vring_used_elem* used_elem) {
    uint32_t i = static_cast<uint16_t>(used_elem->id);
    struct vring_desc* desc = vring_.DescFromIndex(static_cast<uint16_t>(i));

    if (desc->addr != buf_->phys() || desc->len != kBufferSize) {
      fdf::error("{}: entropy response with unexpected buffer", tag());
    } else {
      fdf::debug("{}: received entropy; adding to kernel pool", tag());
      zx_status_t rc = zx_cprng_add_entropy(buf_->virt(), kBufferSize);
      if (rc != ZX_OK) {
        fdf::error("{}: add_entropy failed ({})", tag(), rc);
      }
    }

    vring_.FreeDesc(static_cast<uint16_t>(i));
  };

  // tell the ring to find free chains and hand it back to our lambda
  vring_.IrqRingUpdate(free_chain);
}

void RngDevice::IrqConfigChange() { fdf::debug("{}: Got irq config change (ignoring)", tag()); }

int RngDevice::SeedThreadEntry(void* arg) {
  RngDevice* d = static_cast<RngDevice*>(arg);
  for (;;) {
    zx_status_t rc = d->Request();
    fdf::debug("virtio-rng-seed-thread: RngDevice::Request() returned {}", rc);
    zx_nanosleep(zx_deadline_after(ZX_SEC(300)));
  }
}

zx_status_t RngDevice::Request() {
  fdf::debug("{}: sending entropy request", tag());
  std::lock_guard lock(lock_);
  uint16_t i;
  vring_desc* desc = vring_.AllocDescChain(1, &i);
  if (!desc) {
    fdf::error("{}: failed to allocate descriptor chain of length 1", tag());
    return ZX_ERR_NO_RESOURCES;
  }

  desc->addr = buf_->phys();
  desc->len = kBufferSize;
  desc->flags = VRING_DESC_F_WRITE;
  fdf::debug("{}: allocated descriptor chain desc {:p}, i {}", tag(), (void*)desc, i);

  vring_.SubmitChain(i);
  vring_.Kick();

  fdf::debug("{}: kicked off entropy request", tag());

  return ZX_OK;
}

RngDriver::RngDriver() : fdf::DriverBase2(kDriverName) {}

zx::result<> RngDriver::Start(fdf::DriverContext context) {
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

  device_ = std::make_unique<RngDevice>(std::move(bti), std::move(backend));

  zx_status_t status = device_->Init();
  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok();
}

}  // namespace virtio
