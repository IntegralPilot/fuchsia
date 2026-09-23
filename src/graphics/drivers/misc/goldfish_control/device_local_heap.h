// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_DEVICE_LOCAL_HEAP_H_
#define SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_DEVICE_LOCAL_HEAP_H_

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/ddk/debug.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/function.h>

#include <memory>

#include <fbl/intrusive_double_list.h>

#include "src/graphics/drivers/misc/goldfish_control/heap.h"

namespace goldfish {

class Control;

// LLCPP synchronous server of a goldfish device-local Fuchsia sysmem Heap
// interface.
class DeviceLocalHeap : public Heap {
 public:
  static std::unique_ptr<DeviceLocalHeap> Create(Control* control);

  ~DeviceLocalHeap() override;

  // |fidl::WireServer<fuchsia_sysmem2::Heap>|
  void AllocateVmo(AllocateVmoRequestView request, AllocateVmoCompleter::Sync& completer) override;

  // |fidl::WireServer<fuchsia_sysmem2::Heap>|
  void DeleteVmo(DeleteVmoRequestView request, DeleteVmoCompleter::Sync& completer) override;

  // |Heap|
  void Bind(zx::channel server_request) override;

 private:
  class Buffer {
   public:
    Buffer(DeviceLocalHeap& parent, zx::vmo parent_vmo, BufferKey buffer_key);
    void SetDeleteCompleter(DeleteVmoCompleter::Async async_completer) {
      maybe_delete_completer_.emplace(std::move(async_completer));
    }

   private:
    DeviceLocalHeap& parent_;
    zx::vmo parent_vmo_;
    BufferKey buffer_key_;
    async::Wait wait_deallocate_;
    std::optional<DeleteVmoCompleter::Async> maybe_delete_completer_;
  };

  using BufferSet = std::unordered_map<BufferKey, std::unique_ptr<Buffer>, BufferKeyHash>;
  BufferSet buffers_;

  // This constructor is for internal use only. Use |DeviceLocalHeap::Create()| instead.
  explicit DeviceLocalHeap(Control* control);
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DRIVERS_MISC_GOLDFISH_CONTROL_DEVICE_LOCAL_HEAP_H_
