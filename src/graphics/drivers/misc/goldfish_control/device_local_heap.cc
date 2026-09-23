// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish_control/device_local_heap.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/debug.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/function.h>

#include <cinttypes>
#include <memory>

#include <fbl/intrusive_double_list.h>

#include "src/graphics/drivers/misc/goldfish_control/control_device.h"

namespace goldfish {

namespace {

static const char* kTag = "goldfish-device-local-heap";

fuchsia_hardware_sysmem::wire::HeapProperties GetHeapProperties(fidl::AnyArena& allocator) {
  fuchsia_hardware_sysmem::wire::CoherencyDomainSupport coherency_domain_support(allocator);
  coherency_domain_support.set_cpu_supported(false)
      .set_ram_supported(false)
      .set_inaccessible_supported(true);

  fuchsia_hardware_sysmem::wire::HeapProperties heap_properties(allocator);
  heap_properties.set_coherency_domain_support(allocator, std::move(coherency_domain_support))
      .set_need_clear(false);
  return heap_properties;
}

}  // namespace

DeviceLocalHeap::Buffer::Buffer(DeviceLocalHeap& parent, zx::vmo parent_vmo, BufferKey buffer_key)
    : parent_(parent),
      parent_vmo_(std::move(parent_vmo)),
      buffer_key_(buffer_key),
      wait_deallocate_(parent_vmo_.get(), ZX_VMO_ZERO_CHILDREN, 0) {
  wait_deallocate_.set_handler([this](async_dispatcher_t* dispatcher, async::Wait* wait,
                                      zx_status_t status, const zx_packet_signal_t* signal) {
    auto maybe_local_completer = std::move(maybe_delete_completer_);
    parent_.control()->FreeBufferHandle(buffer_key_);
    if (maybe_local_completer.has_value()) {
      maybe_local_completer->Reply();
    }
  });
  wait_deallocate_.Begin(parent_.loop()->dispatcher());
}

// static
std::unique_ptr<DeviceLocalHeap> DeviceLocalHeap::Create(Control* control) {
  // Using `new` to access a non-public constructor.
  return std::unique_ptr<DeviceLocalHeap>(new DeviceLocalHeap(control));
}

DeviceLocalHeap::DeviceLocalHeap(Control* control) : Heap(control, kTag) {}

DeviceLocalHeap::~DeviceLocalHeap() = default;

void DeviceLocalHeap::AllocateVmo(AllocateVmoRequestView request,
                                  AllocateVmoCompleter::Sync& completer) {
  BufferKey buffer_key(request->buffer_collection_id, request->buffer_index);
  zx::vmo parent_vmo;
  zx_status_t status = zx::vmo::create(request->size, 0, &parent_vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] zx::vmo::create() failed - size: %" PRIu64 " status: %d", kTag,
           request->size, status);
    completer.ReplyError(status);
    return;
  }

  zx::vmo child_vmo;
  status = parent_vmo.create_child(ZX_VMO_CHILD_SLICE, 0, request->size, &child_vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] vmo.create_child() failed: %d", kTag, status);
    completer.ReplyError(status);
    return;
  }

  control()->RegisterBufferHandle(buffer_key);

  auto emplace_result = buffers_.try_emplace(
      buffer_key, std::make_unique<Buffer>(*this, std::move(parent_vmo), buffer_key));
  ZX_ASSERT(emplace_result.second);

  completer.ReplySuccess(std::move(child_vmo));
}

void DeviceLocalHeap::DeleteVmo(DeleteVmoRequestView request, DeleteVmoCompleter::Sync& completer) {
  BufferKey buffer_key(request->buffer_collection_id, request->buffer_index);
  auto iter = buffers_.find(buffer_key);
  // Since this message is only sent by sysmem, and request.vmo is a handle to the child VMO,
  // assert.
  ZX_ASSERT(iter != buffers_.end());
  auto& buffer = iter->second;
  buffer->SetDeleteCompleter(completer.ToAsync());
  // ~request.vmo will trigger ZX_VMO_ZERO_CHILDREN which will Reply() after associated resources
  // are deleted
}

void DeviceLocalHeap::Bind(zx::channel server_request) {
  auto allocator = std::make_unique<fidl::Arena<512>>();
  fuchsia_hardware_sysmem::wire::HeapProperties heap_properties =
      GetHeapProperties(*allocator.get());
  BindWithHeapProperties(std::move(server_request), std::move(allocator),
                         std::move(heap_properties));
}

}  // namespace goldfish
