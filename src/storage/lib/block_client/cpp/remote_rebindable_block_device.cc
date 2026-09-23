// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_client/cpp/remote_rebindable_block_device.h"

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

namespace block_client {

zx_status_t RemoteRebindableBlockDevice::FifoTransaction(BlockFifoRequest* requests, size_t count) {
  return fifo_client_.Transaction(requests, count);
}

zx::result<std::string> RemoteRebindableBlockDevice::GetTopologicalPath() const {
  const fidl::WireResult result = fidl::WireCall(controller_)->GetTopologicalPath();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  fit::result response = result.value();
  if (response.is_error()) {
    return response.take_error();
  }
  return zx::ok(response->path.get());
}

zx::result<> RemoteRebindableBlockDevice::Rebind(std::string_view url_suffix) const {
  const fidl::WireResult result =
      fidl::WireCall(controller_)->Rebind(fidl::StringView::FromExternal(url_suffix));
  if (!result.ok()) {
    return zx::error(result.status());
  }
  fit::result response = result.value();
  if (response.is_error()) {
    return response.take_error();
  }
  return zx::ok();
}

zx_status_t RemoteRebindableBlockDevice::BlockGetInfo(
    fuchsia_storage_block::wire::BlockInfo* out_info) const {
  const fidl::WireResult result = fidl::WireCall(device_)->GetInfo();
  if (!result.ok()) {
    return result.status();
  }
  const fit::result response = result.value();
  if (response.is_error()) {
    return response.error_value();
  }
  *out_info = response.value()->info;
  return ZX_OK;
}

zx_status_t RemoteRebindableBlockDevice::BlockAttachVmo(const zx::vmo& vmo,
                                                        storage::Vmoid* out_vmoid) {
  zx::result vmoid = fifo_client_.RegisterVmo(vmo);
  if (vmoid.is_error()) {
    return vmoid.error_value();
  }
  *out_vmoid = storage::Vmoid(vmoid->TakeId());
  return ZX_OK;
}

zx_status_t RemoteRebindableBlockDevice::VolumeGetInfo(
    fuchsia_storage_block::wire::VolumeManagerInfo* out_manager_info,
    fuchsia_storage_block::wire::VolumeInfo* out_volume_info) const {
  const fidl::WireResult result = fidl::WireCall(device_)->GetVolumeInfo();
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return status;
  }
  *out_manager_info = *response.manager;
  *out_volume_info = *response.volume;
  return ZX_OK;
}

zx_status_t RemoteRebindableBlockDevice::VolumeQuerySlices(
    const uint64_t* slices, size_t slices_count,
    fuchsia_storage_block::wire::VsliceRange* out_ranges, size_t* out_ranges_count) const {
  fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume(device_.channel().borrow());
  const fidl::WireResult result = fidl::WireCall(volume)->QuerySlices(
      fidl::VectorView<uint64_t>::FromExternal(const_cast<uint64_t*>(slices), slices_count));
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    return status;
  }
  std::copy_n(response.response.data(), response.response_count, out_ranges);
  *out_ranges_count = response.response_count;
  return ZX_OK;
}

zx_status_t RemoteRebindableBlockDevice::VolumeExtend(uint64_t offset, uint64_t length) {
  fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume(device_.channel().borrow());
  const fidl::WireResult result = fidl::WireCall(volume)->Extend(offset, length);
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  return response.status;
}

zx_status_t RemoteRebindableBlockDevice::VolumeShrink(uint64_t offset, uint64_t length) {
  fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume(device_.channel().borrow());
  const fidl::WireResult result = fidl::WireCall(volume)->Shrink(offset, length);
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  return response.status;
}

zx::result<std::unique_ptr<RemoteRebindableBlockDevice>> RemoteRebindableBlockDevice::Create(
    fidl::ClientEnd<fuchsia_storage_block::Block> device,
    fidl::ClientEnd<fuchsia_device::Controller> controller) {
  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  if (fidl::Status result = fidl::WireCall(device)->OpenSession(std::move(server)); !result.ok()) {
    return zx::error(result.status());
  }
  const fidl::WireResult result = fidl::WireCall(session)->GetFifo();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  fit::result response = result.value();
  if (response.is_error()) {
    return response.take_error();
  }
  return zx::ok(std::unique_ptr<RemoteRebindableBlockDevice>(new RemoteRebindableBlockDevice(
      std::move(device), std::move(controller), std::move(session), std::move(response->fifo))));
}

RemoteRebindableBlockDevice::RemoteRebindableBlockDevice(
    fidl::ClientEnd<fuchsia_storage_block::Block> device,
    fidl::ClientEnd<fuchsia_device::Controller> controller,
    fidl::ClientEnd<fuchsia_storage_block::Session> session, zx::fifo fifo)
    : device_(std::move(device)),
      controller_(std::move(controller)),
      fifo_client_(std::move(session), std::move(fifo)) {}

fidl::ClientEnd<fuchsia_storage_block::Block> RemoteRebindableBlockDevice::TakeDevice() {
  return std::move(device_);
}

}  // namespace block_client
