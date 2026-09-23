// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/bin/vmm/controller/virtio_mem.h"

#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>

#include <virtio/virtio_ring.h>

namespace {

constexpr auto kComponentName = "virtio_mem";
constexpr auto kComponentCollectionName = "virtio_mem_devices";
constexpr auto kComponentUrl = "#meta/virtio_mem.cm";

}  // namespace

VirtioMem::VirtioMem(const PhysMem& phys_mem, uint64_t pluggable_block_size, uint64_t region_addr,
                     uint64_t region_size)
    : VirtioComponentDevice("Virtio Mem", phys_mem, VIRTIO_MEM_F_UNPLUGGED_INACCESSIBLE,
                            fit::bind_member(this, &VirtioMem::ConfigureQueue),
                            fit::bind_member(this, &VirtioMem::Ready)) {
  config_.block_size = pluggable_block_size;
  config_.addr = region_addr;
  config_.region_size = region_size;
  config_.usable_region_size = region_size;
}

zx_status_t VirtioMem::Start(const zx::guest& guest, ::sys::ComponentContext* context,
                             async_dispatcher_t* dispatcher, uint64_t plugged_block_size,
                             uint64_t region_size) {
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_virtualization_hardware::VirtioMem>::Create();
  fidl::InterfaceRequest<fuchsia::virtualization::hardware::VirtioMem> mem_request(
      server_end.TakeChannel());
  mem_.Bind(std::move(client_end), dispatcher, this);

  zx_status_t status =
      CreateDynamicComponent(context, kComponentCollectionName, kComponentName, kComponentUrl,
                             [mem_request = std::move(mem_request)](
                                 std::shared_ptr<sys::ServiceDirectory> services) mutable {
                               return services->Connect(std::move(mem_request));
                             });
  if (status != ZX_OK) {
    return status;
  }

  fuchsia_virtualization_hardware::wire::StartInfo start_info;
  status = PrepStart(guest, dispatcher, &start_info);
  if (status != ZX_OK) {
    return status;
  }

  uint64_t region_addr;
  {
    std::lock_guard<std::mutex> lock(device_config_.mutex);
    region_addr = config_.addr;
  }
  return mem_.sync()
      ->Start(std::move(start_info), region_addr, plugged_block_size, region_size)
      .status();
}

void VirtioMem::ConnectToMemController(
    fidl::InterfaceRequest<fuchsia::virtualization::MemController> endpoint) {
  bindings_.AddBinding(this, std::move(endpoint));
}

zx_status_t VirtioMem::ConfigureQueue(uint16_t queue, uint16_t size, zx_gpaddr_t desc,
                                      zx_gpaddr_t avail, zx_gpaddr_t used) {
  return mem_.sync()->ConfigureQueue(queue, size, desc, avail, used).status();
}

zx_status_t VirtioMem::Ready(uint32_t negotiated_features) {
  return mem_.sync()->Ready(negotiated_features).status();
}

void VirtioMem::GetMemSize(GetMemSizeCallback callback) {
  uint64_t block_size, region_size, usable_region_size, plugged_size, requested_size;
  {
    std::lock_guard<std::mutex> lock(device_config_.mutex);
    block_size = config_.block_size;
    region_size = config_.region_size;
    usable_region_size = config_.usable_region_size;
    plugged_size = config_.plugged_size;
    requested_size = config_.requested_size;
  }
  callback(block_size, region_size, usable_region_size, plugged_size, requested_size);
}

void VirtioMem::RequestSize(uint64_t requested_size) {
  {
    std::lock_guard<std::mutex> lock(device_config_.mutex);
    config_.requested_size = requested_size;
  }
  // Send a config change interrupt to the guest.
  zx_status_t status = Interrupt(VirtioQueue::SET_CONFIG | VirtioQueue::TRY_INTERRUPT);
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Failed to generate configuration interrupt " << status;
  }
}

void VirtioMem::on_fidl_error(fidl::UnbindInfo error) {
  FX_LOGS(ERROR) << "Connection to VirtioMem lost: " << error;
}

void VirtioMem::OnConfigChanged(
    fidl::WireEvent<fuchsia_virtualization_hardware::VirtioMem::OnConfigChanged>* event) {
  std::lock_guard<std::mutex> lock(device_config_.mutex);
  config_.plugged_size = event->plugged_size;
}
