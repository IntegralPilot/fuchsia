// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/buffers/util.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <lib/zx/vmar.h>

namespace flatland {

fuchsia_sysmem2::BufferUsage get_none_usage() {
  fuchsia_sysmem2::BufferUsage result;
  result.none(fuchsia_sysmem2::kNoneUsage);
  return result;
}

const std::pair<fuchsia_sysmem2::BufferUsage, fuchsia_sysmem2::BufferMemoryConstraints>
GetUsageAndMemoryConstraintsForCpuWriteOften() {
  static const fuchsia_sysmem2::BufferMemoryConstraints kCpuConstraints = [] {
    fuchsia_sysmem2::BufferMemoryConstraints bmc;
    bmc.ram_domain_supported(true);
    bmc.cpu_domain_supported(true);
    return bmc;
  }();
  static const fuchsia_sysmem2::BufferUsage kCpuWriteUsage = [] {
    fuchsia_sysmem2::BufferUsage usage;
    usage.cpu(fuchsia_sysmem2::kCpuUsageWriteOften);
    return usage;
  }();
  return std::make_pair(kCpuWriteUsage, kCpuConstraints);
}

void SetClientConstraintsAndWaitForAllocated(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, uint32_t image_count,
    uint32_t width, uint32_t height, fuchsia_sysmem2::BufferUsage usage,
    const std::vector<fuchsia_images2::PixelFormatModifier>& additional_format_modifiers,
    std::optional<fuchsia_sysmem2::BufferMemoryConstraints> memory_constraints) {
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection;
  {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(token))
            .buffer_collection_request(std::move(server_end))
            .Build());
    FX_DCHECK(result.ok());
    buffer_collection.Bind(std::move(client_end));
  }

  // Use a name with a priority thats > the vulkan implementation, but < what any client would use.
  fuchsia_sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.priority(10u);
  set_name_request.name("FlatlandImage");
  auto set_name_result = buffer_collection->SetName(std::move(set_name_request));
  FX_DCHECK(set_name_result.is_ok());

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  fuchsia_sysmem2::BufferCollectionConstraints constraints;

  if (memory_constraints) {
    constraints.buffer_memory_constraints(std::move(*memory_constraints));
  }
  constraints.usage(std::move(usage));
  constraints.min_buffer_count(image_count);

  size_t image_format_constraints_count =
      1 + static_cast<uint32_t>(additional_format_modifiers.size());
  std::vector<fuchsia_sysmem2::ImageFormatConstraints> image_format_constraints;
  image_format_constraints.reserve(image_format_constraints_count);
  for (size_t i = 0; i < image_format_constraints_count; i++) {
    fuchsia_sysmem2::ImageFormatConstraints ifc;
    ifc.color_spaces(std::vector{fuchsia_images2::ColorSpace::kSrgb});
    ifc.pixel_format(fuchsia_images2::PixelFormat::kR8G8B8A8);
    ifc.pixel_format_modifier(i == 0 ? fuchsia_images2::PixelFormatModifier::kLinear
                                     : additional_format_modifiers[i - 1]);
    ifc.required_min_size(fuchsia_math::SizeU(width, height));
    ifc.required_max_size(fuchsia_math::SizeU(width, height));
    image_format_constraints.push_back(std::move(ifc));
  }
  constraints.image_format_constraints(std::move(image_format_constraints));
  set_constraints_request.constraints(std::move(constraints));

  auto status = buffer_collection->SetConstraints(std::move(set_constraints_request));
  FX_DCHECK(status.is_ok());

  // Have the client wait for allocation.
  auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
  FX_DCHECK(wait_result.is_ok());

  auto release_status = buffer_collection->Release();
  FX_DCHECK(release_status.is_ok());
}

fidl::SyncClient<fuchsia_sysmem2::BufferCollection> CreateBufferCollectionSyncPtrAndSetConstraints(
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, uint32_t image_count,
    uint32_t width, uint32_t height, fuchsia_sysmem2::BufferUsage usage,
    fuchsia_images2::PixelFormat pixel_format,
    std::optional<fuchsia_sysmem2::BufferMemoryConstraints> memory_constraints,
    std::optional<fuchsia_images2::PixelFormatModifier> pixel_format_modifier) {
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection;
  {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
        fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
            .token(std::move(token))
            .buffer_collection_request(std::move(server_end))
            .Build());
    FX_DCHECK(result.ok());
    buffer_collection.Bind(std::move(client_end));
  }
  // Use a name with a priority thats > the vulkan implementation, but < what any client would use.
  fuchsia_sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.priority(10u);
  set_name_request.name("FlatlandClientPointer");
  auto set_name_result = buffer_collection->SetName(std::move(set_name_request));
  FX_DCHECK(set_name_result.is_ok());

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  if (memory_constraints) {
    constraints.buffer_memory_constraints(std::move(*memory_constraints));
  }

  constraints.usage(std::move(usage));
  constraints.min_buffer_count(image_count);

  fuchsia_sysmem2::ImageFormatConstraints image_constraints;

  if (pixel_format_modifier.has_value()) {
    image_constraints.pixel_format_modifier(*pixel_format_modifier);
  }

  switch (pixel_format) {
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      image_constraints.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
      image_constraints.color_spaces(std::vector{fuchsia_images2::ColorSpace::kSrgb});
      break;
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      image_constraints.pixel_format(fuchsia_images2::PixelFormat::kR8G8B8A8);
      image_constraints.color_spaces(std::vector{fuchsia_images2::ColorSpace::kSrgb});
      break;
    case fuchsia_images2::PixelFormat::kI420:
      image_constraints.pixel_format(fuchsia_images2::PixelFormat::kI420);
      image_constraints.color_spaces(std::vector{fuchsia_images2::ColorSpace::kRec709});
      break;
    case fuchsia_images2::PixelFormat::kNv12:
      image_constraints.pixel_format(fuchsia_images2::PixelFormat::kNv12);
      image_constraints.color_spaces(std::vector{fuchsia_images2::ColorSpace::kRec709});
      break;
    default:
      FX_NOTREACHED();
  }

  image_constraints.required_min_size(fuchsia_math::SizeU(width, height));
  image_constraints.required_max_size(fuchsia_math::SizeU(width, height));

  constraints.image_format_constraints(std::vector{std::move(image_constraints)});
  set_constraints_request.constraints(std::move(constraints));

  auto status = buffer_collection->SetConstraints(std::move(set_constraints_request));
  FX_DCHECK(status.is_ok());

  return buffer_collection;
}

namespace {

zx_vm_option_t HostPointerAccessModeToVmoOptions(HostPointerAccessMode host_pointer_access_mode) {
  switch (host_pointer_access_mode) {
    case HostPointerAccessMode::kReadOnly:
      return ZX_VM_PERM_READ;
    case HostPointerAccessMode::kWriteOnly:
    case HostPointerAccessMode::kReadWrite:
      return ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
    default:
      ZX_ASSERT_MSG(false, "Invalid HostPointerAccessMode %u",
                    static_cast<unsigned int>(host_pointer_access_mode));
  }
}

}  // namespace

void MapHostPointer(const fuchsia_sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t*, uint32_t)> callback) {
  if (vmo_idx >= collection_info.buffers().value().size()) {
    callback(nullptr, 0);
    return;
  }

  auto vmo_bytes =
      collection_info.settings().value().buffer_settings().value().size_bytes().value();
  FX_DCHECK(vmo_bytes > 0);

  MapHostPointer(*collection_info.buffers().value()[vmo_idx].vmo(), host_pointer_access_mode,
                 callback, vmo_bytes);
}

void MapHostPointer(const zx::vmo& vmo, HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t* mapped_ptr, uint32_t num_bytes)> callback,
                    uint64_t vmo_bytes) {
  if (vmo_bytes == 0) {
    auto status = vmo.get_prop_content_size(&vmo_bytes);
    // The content size is not always set, so when it's not available,
    // use the full VMO size.
    if (status != ZX_OK || vmo_bytes == 0) {
      vmo.get_size(&vmo_bytes);
    }
    FX_DCHECK(vmo_bytes > 0);
  }

  uint8_t* vmo_host = nullptr;
  const uint32_t vmo_options = HostPointerAccessModeToVmoOptions(host_pointer_access_mode);
  auto status = zx::vmar::root_self()->map(vmo_options, /*vmar_offset*/ 0, vmo, /*vmo_offset*/ 0,
                                           vmo_bytes, reinterpret_cast<uintptr_t*>(&vmo_host));
  FX_DCHECK(status == ZX_OK);

  if (host_pointer_access_mode == HostPointerAccessMode::kReadOnly ||
      host_pointer_access_mode == HostPointerAccessMode::kReadWrite) {
    // Flush the cache before reading back from the host VMO.
    status = zx_cache_flush(vmo_host, vmo_bytes, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    FX_DCHECK(status == ZX_OK);
  }

  callback(vmo_host, static_cast<uint32_t>(vmo_bytes));

  if (host_pointer_access_mode == HostPointerAccessMode::kWriteOnly ||
      host_pointer_access_mode == HostPointerAccessMode::kReadWrite) {
    // Flush the cache after writing to the host VMO.
    status = zx_cache_flush(vmo_host, vmo_bytes, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    FX_DCHECK(status == ZX_OK);
  }

  // Unmap the pointer.
  uintptr_t address = reinterpret_cast<uintptr_t>(vmo_host);
  status = zx::vmar::root_self()->unmap(address, vmo_bytes);
  FX_DCHECK(status == ZX_OK);
}

}  // namespace flatland
