// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_BIN_VMM_CONTROLLER_VIRTIO_MEM_H_
#define SRC_VIRTUALIZATION_BIN_VMM_CONTROLLER_VIRTIO_MEM_H_

#include <fidl/fuchsia.virtualization.hardware/cpp/fidl.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>

#include <virtio/mem.h>
#include <virtio/virtio_ids.h>

#include "src/virtualization/bin/vmm/virtio_device.h"

static constexpr uint16_t kVirtioMemNumQueues = 1;

class VirtioMem
    : public VirtioComponentDevice<VIRTIO_ID_MEM, kVirtioMemNumQueues, virtio_mem_config>,
      public fidl::WireAsyncEventHandler<fuchsia_virtualization_hardware::VirtioMem>,
      public fuchsia::virtualization::MemController {
 public:
  explicit VirtioMem(const PhysMem& phys_mem, uint64_t pluggable_block_size,
                     uint64_t pluggable_region_addr, uint64_t pluggable_region_size);

  zx_status_t Start(const zx::guest& guest, ::sys::ComponentContext* context,
                    async_dispatcher_t* dispatcher, uint64_t plugged_block_size,
                    uint64_t region_size);

  void ConnectToMemController(
      fidl::InterfaceRequest<fuchsia::virtualization::MemController> endpoint);

 private:
  fidl::BindingSet<fuchsia::virtualization::MemController> bindings_;
  fidl::WireSharedClient<fuchsia_virtualization_hardware::VirtioMem> mem_;

  zx_status_t ConfigureQueue(uint16_t queue, uint16_t size, zx_gpaddr_t desc, zx_gpaddr_t avail,
                             zx_gpaddr_t used);
  zx_status_t Ready(uint32_t negotiated_features);
  void on_fidl_error(fidl::UnbindInfo error) override;

  // |fuchsia::virtualization::MemController|
  void GetMemSize(GetMemSizeCallback callback) override;
  void RequestSize(uint64_t requested_size) override;

  // |fuchsia::virtualization::hardware::VirtioMem|
  void OnConfigChanged(
      fidl::WireEvent<fuchsia_virtualization_hardware::VirtioMem::OnConfigChanged>* event) override;
};

#endif  // SRC_VIRTUALIZATION_BIN_VMM_CONTROLLER_VIRTIO_MEM_H_
