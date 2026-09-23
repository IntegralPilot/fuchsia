// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_REMOTE_REBINDABLE_BLOCK_DEVICE_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_REMOTE_REBINDABLE_BLOCK_DEVICE_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/channel.h>
#include <lib/zx/fifo.h>
#include <lib/zx/vmo.h>

#include <memory>
#include <mutex>

#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/block_client/cpp/client.h"

namespace block_client {

// A concrete implementation of |BlockDevice|.
//
// This class is not movable or copyable.
class RemoteRebindableBlockDevice final : public BlockDevice {
 public:
  static zx::result<std::unique_ptr<RemoteRebindableBlockDevice>> Create(
      fidl::ClientEnd<fuchsia_storage_block::Block> device,
      fidl::ClientEnd<fuchsia_device::Controller> controller);
  RemoteRebindableBlockDevice& operator=(RemoteRebindableBlockDevice&&) = delete;
  RemoteRebindableBlockDevice(RemoteRebindableBlockDevice&&) = delete;
  RemoteRebindableBlockDevice& operator=(const RemoteRebindableBlockDevice&) = delete;
  RemoteRebindableBlockDevice(const RemoteRebindableBlockDevice&) = delete;

  zx_status_t FifoTransaction(BlockFifoRequest* requests, size_t count) final;
  zx::result<std::string> GetTopologicalPath() const final;
  zx::result<> Rebind(std::string_view url_suffix) const final;
  zx_status_t BlockGetInfo(fuchsia_storage_block::wire::BlockInfo* out_info) const final;
  zx_status_t BlockAttachVmo(const zx::vmo& vmo, storage::Vmoid* out_vmoid) final;
  zx_status_t VolumeGetInfo(fuchsia_storage_block::wire::VolumeManagerInfo* out_manager_info,
                            fuchsia_storage_block::wire::VolumeInfo* out_volume_info) const final;
  zx_status_t VolumeQuerySlices(const uint64_t* slices, size_t slices_count,
                                fuchsia_storage_block::wire::VsliceRange* out_ranges,
                                size_t* out_ranges_count) const final;
  zx_status_t VolumeExtend(uint64_t offset, uint64_t length) final;
  zx_status_t VolumeShrink(uint64_t offset, uint64_t length) final;

  fidl::ClientEnd<fuchsia_storage_block::Block> TakeDevice();

 private:
  RemoteRebindableBlockDevice(fidl::ClientEnd<fuchsia_storage_block::Block> device,
                              fidl::ClientEnd<fuchsia_device::Controller> controller,
                              fidl::ClientEnd<fuchsia_storage_block::Session> session,
                              zx::fifo fifo);

  fidl::ClientEnd<fuchsia_storage_block::Block> device_;
  fidl::ClientEnd<fuchsia_device::Controller> controller_;
  block_client::Client fifo_client_;
};

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_REMOTE_REBINDABLE_BLOCK_DEVICE_H_
