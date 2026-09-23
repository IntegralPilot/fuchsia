// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/lib/usb-endpoint/include/usb-endpoint/usb-endpoint-client.h"

#include <lib/zx/vmar.h>

#include <mutex>

namespace usb::internal {

namespace {

zx::result<std::optional<uint64_t>> GetMappedKey(
    const fuchsia_hardware_usb_request::Buffer& buffer) {
  switch (buffer.Which()) {
    case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId:
      return zx::ok(buffer.vmo_id().value());
    case fuchsia_hardware_usb_request::Buffer::Tag::kData:
      // Is not unmapped at this point.
      return zx::ok(std::nullopt);
    default:
      zxlogf(ERROR, "Unrecognized buffer type %lu", static_cast<unsigned long>(buffer.Which()));
      return zx::error(ZX_ERR_INTERNAL);
  }
}

}  // namespace

EndpointClientBase::~EndpointClientBase() {
  // Note that VMOs should be unpinned when returned from drivers!
  while (auto req = free_reqs_.Remove()) {
    auto status = DeleteRequest(std::move(*req));
    if (status != ZX_OK) {
      zxlogf(ERROR, "Could not delete request %d", status);
    }
  }

  // Unmap all remaining buffers. These entries should only exist due to outstanding
  // usb requests that we will no longer need to access after this endpoint client is
  // destructed anyways.
  std::map vmo_mapped_addrs = std::move(vmo_mapped_addrs_);
  for (auto& [_, mapped] : vmo_mapped_addrs) {
    zx::vmar::root_self()->unmap(mapped.addr, mapped.size);
  }
}

zx_status_t EndpointClientBase::Unmap(const fuchsia_hardware_usb_request::BufferRegion& buffer) {
  auto key = GetMappedKey(*buffer.buffer());
  if (key.is_error()) {
    return key.error_value();
  }
  if (!key.value()) {
    return ZX_OK;
  }

  auto node = vmo_mapped_addrs_.extract(*key.value());
  if (node.empty()) {
    return ZX_OK;
  }
  auto mapped = node.mapped();
  auto status = zx::vmar::root_self()->unmap(mapped.addr, mapped.size);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to unmap VMO %d", status);
    return status;
  }

  return ZX_OK;
}

size_t EndpointClientBase::AddRequests(size_t req_count, size_t size,
                                       fuchsia_hardware_usb_request::Buffer::Tag type) {
  switch (type) {
    case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId:
      return RegisterVmos(req_count, size);
    case fuchsia_hardware_usb_request::Buffer::Tag::kData:
      for (size_t i = 0; i < req_count; i++) {
        free_reqs_.Add(std::move(usb::FidlRequest(ep_type_).add_data(std::vector<uint8_t>(size))));
      }
      return req_count;
    default:
      return 0;
  }
}

zx_status_t EndpointClientBase::DeleteRequest(usb::FidlRequest&& request) {
  zx_status_t ret_status = ZX_OK;
  std::vector<uint64_t> vmo_ids;
  for (auto& d : *request->data()) {
    auto status = Unmap(d);
    if (status != ZX_OK) {
      zxlogf(ERROR, "Could not unmap buffer region %d", status);
      // Return the latest failed status value, but keep trying to unmap the rest of the buffer
      // regions
      ret_status = status;
    }

    if (d.buffer()->Which() == fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) {
      vmo_ids.push_back(d.buffer()->vmo_id().value());
    }
  }

  if (!vmo_ids.empty()) {
    fidl::WireResult result = client_.wire_sync()->UnregisterVmos(
        fidl::VectorView<uint64_t>::FromExternal(vmo_ids.data(), vmo_ids.size()));
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to unregister vmo %s", result.FormatDescription().c_str());
      return result.status();
    }
    if (result->failed_vmo_ids.size() != result->errors.size()) {
      zxlogf(ERROR, "Inconsistent failed vmo ids and errors");
      return ZX_ERR_INTERNAL;
    }
    for (size_t i = 0; i < result->failed_vmo_ids.size(); i++) {
      zx_status_t status = result->errors.at(i);
      zxlogf(ERROR, "Failed to unregister vmo %lu: %s", result->failed_vmo_ids.at(i),
             zx_status_get_string(status));
      ret_status = status;
    }
  }

  return ret_status;
}

zx_status_t EndpointClientBase::Close() {
  zx_status_t ret_status = ZX_OK;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    while (std::optional<usb::FidlRequest> req = free_reqs_.Remove()) {
      for (fuchsia_hardware_usb_request::BufferRegion& d : *req.value()->data()) {
        zx_status_t status = Unmap(d);
        if (status != ZX_OK) {
          zxlogf(ERROR, "Could not unmap buffer region %d", status);
          // Return the latest failed status value, but keep trying to unmap the rest of the buffer
          // regions
          ret_status = status;
        }
      }
    }
  }
  client_.AsyncTeardown();
  return ret_status;
}

size_t EndpointClientBase::RegisterVmos(size_t vmo_count, size_t vmo_size) {
  fidl::Arena arena;
  fidl::VectorView<fuchsia_hardware_usb_endpoint::wire::VmoInfo> vmo_info(arena, vmo_count);
  for (uint32_t i = 0; i < vmo_count; i++) {
    vmo_info.at(i) = fuchsia_hardware_usb_endpoint::wire::VmoInfo::Builder(arena)
                         .id(buffer_id_++)
                         .size(vmo_size)
                         .Build();
  }

  size_t actual = 0;
  fidl::WireResult result = client_.wire_sync()->RegisterVmos(vmo_info);
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to register VMOs %s", result.FormatDescription().c_str());
    return 0;
  }

  fidl::VectorView<fuchsia_hardware_usb_endpoint::wire::VmoHandle>& vmos = result->vmos;
  actual = vmos.size();
  for (const auto& vmo : vmos) {
    free_reqs_.Add(std::move(usb::FidlRequest(ep_type_).add_vmo_id(vmo.id(), vmo_size)));

    zx_vaddr_t mapped_addr;
    auto status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo.vmo(), 0,
                                             vmo_size, &mapped_addr);
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to map the vmo: %d", status);
      // Try for the next one.
      continue;
    }
    std::lock_guard<std::mutex> _(mutex());
    vmo_mapped_addrs_.emplace(vmo.id(), usb::MappedVmo{
                                            .addr = mapped_addr,
                                            .size = vmo_size,
                                        });
  }
  return actual;
}

zx::result<std::optional<usb::MappedVmo>> EndpointClientBase::get_mapped(
    const fuchsia_hardware_usb_request::Buffer& buffer) {
  auto key = GetMappedKey(buffer);
  if (key.is_error()) {
    return zx::error(key.error_value());
  }
  if (!key.value()) {
    return zx::ok(std::nullopt);
  }

  return zx::ok(vmo_mapped_addrs_.at(*key.value()));
}

}  // namespace usb::internal
