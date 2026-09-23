// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/misc/goldfish_control/host_visible_heap.h"

#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/trace/event.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/fpromise/result.h>
#include <lib/zx/vmar.h>
#include <zircon/assert.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <memory>

#include <fbl/algorithm.h>

#include "src/graphics/drivers/misc/goldfish_control/control_device.h"

namespace goldfish {

namespace {

static const char* kTag = "goldfish-host-visible-heap";

fuchsia_hardware_sysmem::wire::HeapProperties GetHeapProperties(fidl::AnyArena& allocator) {
  fuchsia_hardware_sysmem::wire::CoherencyDomainSupport coherency_domain_support(allocator);
  coherency_domain_support.set_cpu_supported(true)
      .set_ram_supported(true)
      .set_inaccessible_supported(false);

  fuchsia_hardware_sysmem::wire::HeapProperties heap_properties(allocator);
  heap_properties
      .set_coherency_domain_support(allocator, std::move(coherency_domain_support))
      // Allocated VMOs are not directly writeable since they are physical
      // VMOs on MMIO; Also, contents of VMOs allocated by this Heap are only
      // valid after |CreateColorBuffer()| render control call. Thus it doesn't
      // work for sysmem to clear the VMO contents, instead we do map-and-clear
      // in the end of |CreateResource()|.
      .set_need_clear(false);
  return heap_properties;
}

zx_status_t CheckSingleBufferSettings(
    const fuchsia_sysmem2::wire::SingleBufferSettings& single_buffer_settings) {
  bool has_image_format_constraints = single_buffer_settings.has_image_format_constraints();
  bool has_buffer_settings = single_buffer_settings.has_buffer_settings();

  if (!has_buffer_settings && !has_image_format_constraints) {
    zxlogf(ERROR,
           "[%s][%s] Both buffer_settings and image_format_constraints "
           "are missing, SingleBufferSettings is invalid.",
           kTag, __func__);
    return ZX_ERR_INVALID_ARGS;
  }

  if (has_image_format_constraints) {
    const auto& image_constraints = single_buffer_settings.image_format_constraints();
    if (!image_constraints.has_pixel_format() || !image_constraints.has_min_size()) {
      zxlogf(ERROR, "[%s][%s] image_constraints missing arguments: pixel_format %d min_size %d",
             kTag, __func__, image_constraints.has_pixel_format(),
             image_constraints.has_min_size());
      return ZX_ERR_INVALID_ARGS;
    }
  }

  if (has_buffer_settings) {
    const auto& buffer_settings = single_buffer_settings.buffer_settings();
    if (!buffer_settings.has_size_bytes()) {
      zxlogf(ERROR, "[%s][%s] buffer_settings missing arguments: size %d", kTag, __func__,
             buffer_settings.has_size_bytes());
      return ZX_ERR_INVALID_ARGS;
    }
  }

  return ZX_OK;
}

fpromise::result<fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params, zx_status_t>
GetCreateColorBuffer2Params(fidl::AnyArena& allocator,
                            const fuchsia_sysmem2::wire::SingleBufferSettings& buffer_settings,
                            uint64_t paddr) {
  using fuchsia_hardware_goldfish::wire::ColorBufferFormatType;
  using fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params;
  using fuchsia_images2::wire::PixelFormat;

  ZX_DEBUG_ASSERT(buffer_settings.has_image_format_constraints());
  const auto& image_constraints = buffer_settings.image_format_constraints();

  ZX_DEBUG_ASSERT(image_constraints.has_pixel_format() && image_constraints.has_min_size());

  // TODO(https://fxbug.dev/42137913): Support other pixel formats.
  const auto& pixel_format = image_constraints.pixel_format();
  ColorBufferFormatType color_buffer_format;
  switch (pixel_format) {
    case PixelFormat::kB8G8R8A8:
      color_buffer_format = ColorBufferFormatType::kBgra;
      break;
    case PixelFormat::kR8G8B8A8:
      color_buffer_format = ColorBufferFormatType::kRgba;
      break;
    case PixelFormat::kR8:
      color_buffer_format = ColorBufferFormatType::kLuminance;
      break;
    case PixelFormat::kR8G8:
      color_buffer_format = ColorBufferFormatType::kRg;
      break;
    default:
      zxlogf(ERROR, "[%s][%s] pixel_format unsupported: type %u", __func__, kTag,
             static_cast<uint32_t>(pixel_format));
      return fpromise::error(ZX_ERR_NOT_SUPPORTED);
  }

  uint32_t width = image_constraints.min_size().width;
  if (image_constraints.has_required_max_size()) {
    width = std::max(width, image_constraints.required_max_size().width);
  }
  width = fbl::round_up(
      width, image_constraints.has_size_alignment() ? image_constraints.size_alignment().width : 1);

  uint32_t height = image_constraints.min_size().height;
  if (image_constraints.has_required_max_size()) {
    height = std::max(height, image_constraints.required_max_size().height);
  }
  height = fbl::round_up(height, image_constraints.has_size_alignment()
                                     ? image_constraints.size_alignment().height
                                     : 1);

  CreateColorBuffer2Params buffer2_params(allocator);
  buffer2_params.set_width(width)
      .set_height(height)
      .set_memory_property(fuchsia_hardware_goldfish::wire::kMemoryPropertyHostVisible)
      .set_physical_address(allocator, paddr)
      .set_format(color_buffer_format);
  return fpromise::ok(std::move(buffer2_params));
}

}  // namespace

HostVisibleHeap::Block::Block(zx::vmo vmo, uint64_t paddr,
                              fit::function<void(Block&)> deallocate_callback,
                              async_dispatcher_t* dispatcher)
    : vmo(std::move(vmo)),
      paddr(paddr),
      wait_deallocate(this->vmo.get(), ZX_VMO_ZERO_CHILDREN, 0u,
                      [this, deallocate_callback = std::move(deallocate_callback)](
                          async_dispatcher_t* dispatcher, async::Wait* wait, zx_status_t status,
                          const zx_packet_signal_t* signal) { deallocate_callback(*this); }) {
  wait_deallocate.Begin(dispatcher);
}

// static
std::unique_ptr<HostVisibleHeap> HostVisibleHeap::Create(Control* control) {
  // Using `new` to access a non-public constructor.
  return std::unique_ptr<HostVisibleHeap>(new HostVisibleHeap(control));
}

HostVisibleHeap::HostVisibleHeap(Control* control) : Heap(control, kTag) {}

HostVisibleHeap::~HostVisibleHeap() = default;

void HostVisibleHeap::AllocateVmo(AllocateVmoRequestView request,
                                  AllocateVmoCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "HostVisibleHeap::AllocateVmo", "size", request->size);
  BufferKey buffer_key(request->buffer_collection_id, request->buffer_index);

  auto result = control()->address_space_child()->AllocateBlock(request->size);
  if (!result.ok()) {
    zxlogf(ERROR, "[%s] AllocateBlock FIDL call failed: status %d", kTag, result.status());
    completer.ReplyError(result.status());
    return;
  }
  if (result.value().res != ZX_OK) {
    zxlogf(ERROR, "[%s] AllocateBlock failed: res %d", kTag, result.value().res);
    completer.ReplyError(result.status());
    return;
  }

  // Get |paddr| of the |Block| to use in Buffer create params.
  uint64_t paddr = result.value().paddr;

  // We need to clean up the allocated block if |zx_vmo_create_child| or
  // |fsl::GetKoid| fails, which could happen before we create and bind the
  // |DeallocateBlock()| wait in |Block|.
  auto cleanup_block = fit::defer([this, paddr] {
    auto result = control()->address_space_child()->DeallocateBlock(paddr);
    if (!result.ok()) {
      zxlogf(ERROR, "[%s] DeallocateBlock FIDL call failed: status %d", kTag, result.status());
    } else if (result.value().res != ZX_OK) {
      zxlogf(ERROR, "[%s] DeallocateBlock failed: res %d", kTag, result.value().res);
    }
  });

  zx::vmo parent_vmo = std::move(result.value().vmo);
  zx::vmo child_vmo;
  zx_status_t status =
      parent_vmo.create_child(ZX_VMO_CHILD_SLICE, /*offset=*/0u, request->size, &child_vmo);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] zx_vmo_create_child failed: %d", kTag, status);
    completer.Close(status);
    return;
  }

  using fuchsia_hardware_goldfish::wire::ColorBufferFormatType;
  using fuchsia_hardware_goldfish::wire::CreateColorBuffer2Params;

  ZX_DEBUG_ASSERT(child_vmo.is_valid());

  status = CheckSingleBufferSettings(request->settings);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] Invalid single buffer settings", kTag);
    completer.ReplyError(status);
    return;
  }

  bool is_image = request->settings.has_image_format_constraints();
  if (!is_image) {
    zxlogf(ERROR, "Creating a host-visible data buffer is not supported.");
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  TRACE_DURATION(
      "gfx", "HostVisibleHeap::CreateResource", "type", is_image ? "image" : "buffer",
      "image:width", is_image ? request->settings.image_format_constraints().min_size().width : 0,
      "image:height", is_image ? request->settings.image_format_constraints().min_size().height : 0,
      "image:format",
      is_image ? static_cast<uint32_t>(request->settings.image_format_constraints().pixel_format())
               : 0,
      "buffer:size", is_image ? 0 : request->settings.buffer_settings().size_bytes());

  // Register buffer handle for VMO.
  control()->RegisterBufferHandle(buffer_key);

  // If the following part fails, we need to free the ColorBuffer/Buffer
  // handle so that there is no handle/resource leakage.
  auto cleanup_handle = fit::defer([this, buffer_key] { control()->FreeBufferHandle(buffer_key); });

  fidl::Arena allocator;
  // ColorBuffer creation.
  auto create_params = GetCreateColorBuffer2Params(allocator, request->settings, paddr);
  if (create_params.is_error()) {
    completer.ReplyError(create_params.error());
    return;
  }

  // Create actual ColorBuffer and map physical address |paddr| to
  // address of the ColorBuffer's host memory.
  auto color_buffer_result =
      control()->CreateColorBuffer2(child_vmo, buffer_key, create_params.take_value());
  if (color_buffer_result.is_error()) {
    zxlogf(ERROR, "[%s] CreateColorBuffer error: status %d", kTag, status);
    completer.Close(color_buffer_result.error());
    return;
  }
  if (color_buffer_result.value().res != ZX_OK) {
    zxlogf(ERROR, "[%s] CreateColorBuffer2 failed: res = %d", kTag,
           color_buffer_result.value().res);
    completer.ReplyError(color_buffer_result.value().res);
    return;
  }

  // Host visible ColorBuffer should have page offset of zero, otherwise
  // part of the page mapped from address space device not used for the
  // ColorBuffer can be leaked.
  ZX_DEBUG_ASSERT(color_buffer_result.value().hw_address_page_offset == 0u);

  // Heap should fill VMO with zeroes before returning it to clients.
  // Since VMOs allocated by address space device are physical VMOs not
  // supporting zx_vmo_write, we map it and fill the mapped memory address
  // with zero.
  uint64_t vmo_size;
  status = child_vmo.get_size(&vmo_size);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] zx_vmo_get_size failed: %d", kTag, status);
    completer.Close(status);
    return;
  }

  zx_paddr_t addr;
  status =
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, /*vmar_offset=*/0u, child_vmo,
                                 /*vmo_offset=*/0u, vmo_size, &addr);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] zx_vmar_map failed: %d", kTag, status);
    completer.Close(status);
    return;
  }

  memset(reinterpret_cast<uint8_t*>(addr), 0u, vmo_size);

  status = zx::vmar::root_self()->unmap(addr, vmo_size);
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] zx_vmar_unmap failed: %d", kTag, status);
    completer.Close(status);
    return;
  }

  auto [iter, emplace_success] = blocks_.try_emplace(
      buffer_key, std::move(parent_vmo), result.value().paddr,
      [this, buffer_key](Block& block) {
        auto maybe_local_completer = std::move(block.maybe_delete_completer);
        // This destroys the color buffer associated with |id| and frees the color
        // buffer handle |id|.
        control()->FreeBufferHandle(buffer_key);
        DeallocateVmo(buffer_key);
        if (maybe_local_completer.has_value()) {
          maybe_local_completer->Reply();
        }
      },
      loop()->dispatcher());
  ZX_ASSERT(emplace_success);

  // Everything is done, now we can cancel the cleanup auto calls. The Wait completion will clean up
  // later.
  cleanup_handle.cancel();
  cleanup_block.cancel();

  completer.ReplySuccess(std::move(child_vmo));
}

void HostVisibleHeap::DeleteVmo(DeleteVmoRequestView request, DeleteVmoCompleter::Sync& completer) {
  BufferKey buffer_key(request->buffer_collection_id, request->buffer_index);
  auto iter = blocks_.find(buffer_key);
  // The incoming vmo is a handle to a child VMO of the parent vmo in Block, so the Block is
  // expected to still be in blocks_. Since this message is only sent by sysmem, assert.
  ZX_ASSERT(iter != blocks_.end());
  auto& block = iter->second;
  // complete async
  block.maybe_delete_completer.emplace(completer.ToAsync());
  // ~request.vmo will shortly trigger ZX_VMO_ZERO_CHILDREN; Block will notice and call Reply()
}

void HostVisibleHeap::DeallocateVmo(BufferKey buffer_key) {
  TRACE_DURATION("gfx", "HostVisibleHeap::DeallocateVmo");

  ZX_DEBUG_ASSERT(blocks_.find(buffer_key) != blocks_.end());

  auto iter = blocks_.find(buffer_key);
  auto& block = iter->second;
  uint64_t paddr = block.paddr;
  blocks_.erase(iter);

  auto result = control()->address_space_child()->DeallocateBlock(paddr);
  if (!result.ok()) {
    zxlogf(ERROR, "[%s] DeallocateBlock FIDL call error: status %d", kTag, result.status());
  } else if (result.value().res != ZX_OK) {
    zxlogf(ERROR, "[%s] DeallocateBlock failed: res %d", kTag, result.value().res);
  }
}

void HostVisibleHeap::Bind(zx::channel server_request) {
  auto allocator = std::make_unique<fidl::Arena<512>>();
  fuchsia_hardware_sysmem::wire::HeapProperties heap_properties =
      GetHeapProperties(*allocator.get());
  BindWithHeapProperties(std::move(server_request), std::move(allocator),
                         std::move(heap_properties));
}

}  // namespace goldfish
