// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_client/cpp/remote_block_device.h"

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

namespace block_client {

zx_status_t RemoteBlockDevice::FifoTransaction(BlockFifoRequest* requests, size_t count) {
  return fifo_client_.Transaction(requests, count);
}

zx::result<std::string> RemoteBlockDevice::GetTopologicalPath() const {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> RemoteBlockDevice::Rebind(std::string_view url_suffix) const {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t RemoteBlockDevice::BlockGetInfo(
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

zx_status_t RemoteBlockDevice::BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out_vmoid) {
  zx::result vmoid = fifo_client_.RegisterVmo(vmo);
  if (vmoid.is_error()) {
    return vmoid.error_value();
  }
  *out_vmoid = storage::Vmoid(vmoid->TakeId());
  return ZX_OK;
}

zx_status_t RemoteBlockDevice::VolumeGetInfo(
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

zx_status_t RemoteBlockDevice::VolumeQuerySlices(
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

zx_status_t RemoteBlockDevice::VolumeExtend(uint64_t offset, uint64_t length) {
  fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume(device_.channel().borrow());
  const fidl::WireResult result = fidl::WireCall(volume)->Extend(offset, length);
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  return response.status;
}

zx_status_t RemoteBlockDevice::VolumeShrink(uint64_t offset, uint64_t length) {
  fidl::UnownedClientEnd<fuchsia_storage_block::Block> volume(device_.channel().borrow());
  const fidl::WireResult result = fidl::WireCall(volume)->Shrink(offset, length);
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  return response.status;
}

zx::result<std::unique_ptr<RemoteBlockDevice>> RemoteBlockDevice::Create(
    fidl::ClientEnd<fuchsia_storage_block::Block> device) {
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
  return zx::ok(std::unique_ptr<RemoteBlockDevice>(
      new RemoteBlockDevice(std::move(device), std::move(session), std::move(response->fifo))));
}

RemoteBlockDevice::RemoteBlockDevice(fidl::ClientEnd<fuchsia_storage_block::Block> device,
                                     fidl::ClientEnd<fuchsia_storage_block::Session> session,
                                     zx::fifo fifo)
    : device_(std::move(device)), fifo_client_(std::move(session), std::move(fifo)) {}

zx_status_t ReadWriteBlocks(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                            void* buffer, size_t buffer_length, size_t offset, bool write) {
  // Get the Block info for block size calculations:
  const fidl::WireResult get_info_result = fidl::WireCall(device)->GetInfo();
  if (!get_info_result.ok()) {
    return get_info_result.status();
  }
  const fit::result response = get_info_result.value();
  if (response.is_error()) {
    return response.error_value();
  }

  size_t block_size = response.value()->info.block_size;
  if (!buffer || buffer_length % block_size != 0 || offset % block_size != 0 ||
      buffer_length / block_size > std::numeric_limits<uint32_t>::max()) {
    return ZX_ERR_INVALID_ARGS;
  }

  // The Fifo API disallows zero length.
  if (buffer_length == 0)
    return ZX_OK;

  zx::vmo vmo;
  if (zx_status_t status = zx::vmo::create(buffer_length, 0, &vmo); status != ZX_OK) {
    return status;
  }

  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  const fidl::OneWayStatus status = fidl::WireCall(device)->OpenSession(std::move(server));
  if (!status.ok())
    return status.status();

  uint16_t vmo_id;
  {
    zx::vmo duplicate;
    if (zx_status_t status = vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicate); status != ZX_OK)
      return status;
    const fidl::WireResult result = fidl::WireCall(session)->AttachVmo(std::move(duplicate));
    if (!result.ok())
      return result.status();
    const fit::result response = result.value();
    if (response.is_error())
      return response.error_value();
    vmo_id = response->vmoid.id;
  }

  zx::fifo fifo;
  {
    const fidl::WireResult result = fidl::WireCall(session)->GetFifo();
    if (!result.ok())
      return result.status();
    const fit::result response = result.value();
    if (response.is_error())
      return response.error_value();
    fifo = std::move(response->fifo);
  }

  static std::atomic<uint32_t> counter;
  BlockFifoRequest request = {
      .command =
          {
              .opcode = static_cast<uint8_t>(write ? BLOCK_OPCODE_WRITE : BLOCK_OPCODE_READ),
          },
      .reqid = ++counter,
      .vmoid = vmo_id,
      .length = static_cast<uint32_t>(buffer_length / block_size),
      .dev_offset = offset / block_size,
  };

  if (write) {
    if (zx_status_t status = vmo.write(buffer, 0, buffer_length); status != ZX_OK)
      return status;
  }

  size_t actual;
  if (zx_status_t status = fifo.write(sizeof(BlockFifoRequest), &request, 1, &actual);
      status != ZX_OK) {
    return status;
  }

  BlockFifoResponse fifo_response;
  zx_signals_t signals;
  fifo.wait_one(ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED, zx::time::infinite(), &signals);
  if (zx_status_t status = fifo.read(sizeof(BlockFifoResponse), &fifo_response, 1, &actual);
      status != ZX_OK) {
    return status;
  }
  if (fifo_response.reqid != request.reqid)
    return ZX_ERR_INTERNAL;
  if (fifo_response.status != ZX_OK)
    return fifo_response.status;

  if (!write) {
    return vmo.read(buffer, 0, buffer_length);
  }
  return ZX_OK;
}

fidl::ClientEnd<fuchsia_storage_block::Block> RemoteBlockDevice::TakeDevice() {
  return std::move(device_);
}

zx_status_t SingleReadBytes(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                            void* buffer, size_t buffer_size, size_t offset) {
  return ReadWriteBlocks(device, buffer, buffer_size, offset, false);
}

zx_status_t SingleWriteBytes(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                             void* buffer, size_t buffer_size, size_t offset) {
  return ReadWriteBlocks(device, buffer, buffer_size, offset, true);
}
}  // namespace block_client
