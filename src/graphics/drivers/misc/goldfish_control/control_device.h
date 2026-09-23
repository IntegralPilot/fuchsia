// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_CONTROL_DEVICE_H_
#define SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_CONTROL_DEVICE_H_

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/markers.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/device.h>
#include <lib/ddk/io-buffer.h>
#include <lib/fpromise/result.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <zircon/types.h>

#include <map>
#include <vector>

#include <ddktl/device.h>
#include <fbl/condition_variable.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/mutex.h>

#include "src/graphics/drivers/misc/goldfish_control/heap.h"

namespace goldfish {

class Control;
using ControlType =
    ddk::Device<Control, ddk::Messageable<fuchsia_hardware_goldfish::ControlDevice>::Mixin>;

class Control : public ControlType {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent);

  explicit Control(zx_device_t* parent, async_dispatcher_t* dispatcher);
  ~Control();

  zx_status_t Bind();

  void RegisterBufferHandle(BufferKey buffer_key);
  void FreeBufferHandle(BufferKey buffer_key);

  using CreateColorBuffer2Result = fpromise::result<
      fidl::WireResponse<fuchsia_hardware_goldfish::ControlDevice::CreateColorBuffer2>,
      zx_status_t>;

  CreateColorBuffer2Result CreateColorBuffer2(
      const zx::vmo& vmo, BufferKey buffer_key,
      fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params create_params);

  // |fidl::WireServer<fuchsia_hardware_goldfish::ControlDevice>|
  void CreateColorBuffer2(CreateColorBuffer2RequestView request,
                          CreateColorBuffer2Completer::Sync& completer) override;

  // |fidl::WireServer<fuchsia_hardware_goldfish::ControlDevice>|
  void CreateBuffer2(CreateBuffer2RequestView request,
                     CreateBuffer2Completer::Sync& completer) override;

  // |fidl::WireServer<fuchsia_hardware_goldfish::ControlDevice>|
  void CreateSyncFence(CreateSyncFenceRequestView request,
                       CreateSyncFenceCompleter::Sync& completer) override;

  // |fidl::WireServer<fuchsia_hardware_goldfish::ControlDevice>|
  void GetBufferHandle(GetBufferHandleRequestView request,
                       GetBufferHandleCompleter::Sync& completer) override;

  // |fidl::WireServer<fuchsia_hardware_goldfish::ControlDevice>|
  void GetBufferHandleInfo(GetBufferHandleInfoRequestView request,
                           GetBufferHandleInfoCompleter::Sync& completer) override;

  // Device protocol implementation.
  void DdkRelease();

  // Used by heaps. Removes a specific heap from the linked list.
  void RemoveHeap(Heap* heap);

  fidl::WireSyncClient<fuchsia_hardware_goldfish::AddressSpaceChildDriver>& address_space_child() {
    return address_space_child_;
  }

 private:
  zx_status_t Init();

  zx_status_t InitAddressSpaceDeviceLocked() TA_REQ(lock_);
  zx_status_t InitPipeDeviceLocked() TA_REQ(lock_);
  zx_status_t InitSyncDeviceLocked() TA_REQ(lock_);

  // TODO(https://fxbug.dev/42161642): Remove these pipe IO functions and use
  // //src/devices/lib/goldfish/pipe_io instead.
  int32_t WriteLocked(uint32_t cmd_size, int32_t* consumed_size) TA_REQ(lock_);
  void WriteLocked(uint32_t cmd_size) TA_REQ(lock_);
  zx_status_t ReadResultLocked(void* result, size_t size) TA_REQ(lock_);
  zx_status_t ReadResultLocked(uint32_t* result) TA_REQ(lock_) {
    return ReadResultLocked(result, sizeof(uint32_t));
  }
  zx_status_t ExecuteCommandLocked(uint32_t cmd_size, uint32_t* result) TA_REQ(lock_);
  zx_status_t CreateColorBufferLocked(uint32_t width, uint32_t height, uint32_t format,
                                      uint32_t* id) TA_REQ(lock_);
  void CloseBufferOrColorBufferLocked(uint32_t id) TA_REQ(lock_);
  void CloseBufferLocked(uint32_t id) TA_REQ(lock_);
  void CloseColorBufferLocked(uint32_t id) TA_REQ(lock_);
  zx_status_t SetColorBufferVulkanModeLocked(uint32_t id, uint32_t mode, uint32_t* result)
      TA_REQ(lock_);
  zx_status_t SetColorBufferVulkanMode2Locked(uint32_t id, uint32_t mode, uint32_t memory_property,
                                              uint32_t* result) TA_REQ(lock_);
  zx_status_t MapGpaToBufferHandleLocked(uint32_t id, uint64_t gpa, uint64_t size, uint32_t* result)
      TA_REQ(lock_);
  zx_status_t CreateSyncKHRLocked(uint64_t* glsync_out, uint64_t* syncthread_out) TA_REQ(lock_);

  fit::result<zx_status_t, BufferKey> GetBufferKeyForVmo(const zx::vmo& vmo);

  fuchsia_hardware_goldfish_pipe::Service::InstanceHandler
  CreateGoldfishPipeServiceInstanceHandler();

  fbl::Mutex lock_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish::AddressSpaceDevice> address_space_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish::SyncDevice> sync_;
  fidl::SyncClient<fuchsia_sysmem2::Allocator> sysmem_;
  int32_t id_ = 0;
  zx::bti bti_ TA_GUARDED(lock_);
  ddk::IoBuffer cmd_buffer_ TA_GUARDED(lock_);
  ddk::IoBuffer io_buffer_ TA_GUARDED(lock_);

  fbl::DoublyLinkedList<std::unique_ptr<Heap>> heaps_ TA_GUARDED(lock_);
  std::vector<std::unique_ptr<Heap>> removed_heaps_;

  zx::event pipe_event_;

  fidl::WireSyncClient<fuchsia_hardware_goldfish::AddressSpaceChildDriver> address_space_child_;
  fidl::WireSyncClient<fuchsia_hardware_goldfish::SyncTimeline> sync_timeline_;

  // TODO(https://fxbug.dev/42107181): This should be std::unordered_map.
  //
  // buffer_collection_id, buffer_index
  std::map<BufferKey, uint32_t> buffer_handles_ TA_GUARDED(lock_);

  struct BufferHandleInfo {
    fuchsia_hardware_goldfish::wire::BufferHandleType type;
    uint32_t memory_property;
  };
  std::map<uint32_t, BufferHandleInfo> buffer_handle_info_ TA_GUARDED(lock_);

  // The outgoing services are dispatched onto `dispatcher_`.
  async_dispatcher_t* dispatcher_;

  component::OutgoingDirectory outgoing_;
  fidl::ServerBindingGroup<fuchsia_hardware_goldfish::ControlDevice> bindings_;

  DISALLOW_COPY_ASSIGN_AND_MOVE(Control);
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_CONTROL_DEVICE_H_
