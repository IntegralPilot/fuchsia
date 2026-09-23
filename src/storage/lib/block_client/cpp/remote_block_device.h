// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_REMOTE_BLOCK_DEVICE_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_REMOTE_BLOCK_DEVICE_H_

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
class RemoteBlockDevice final : public BlockDevice {
 public:
  static zx::result<std::unique_ptr<RemoteBlockDevice>> Create(
      fidl::ClientEnd<fuchsia_storage_block::Block> device);
  RemoteBlockDevice& operator=(RemoteBlockDevice&&) = delete;
  RemoteBlockDevice(RemoteBlockDevice&&) = delete;
  RemoteBlockDevice& operator=(const RemoteBlockDevice&) = delete;
  RemoteBlockDevice(const RemoteBlockDevice&) = delete;

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
  RemoteBlockDevice(fidl::ClientEnd<fuchsia_storage_block::Block> device,
                    fidl::ClientEnd<fuchsia_storage_block::Session> session, zx::fifo fifo);

  fidl::ClientEnd<fuchsia_storage_block::Block> device_;
  block_client::Client fifo_client_;
};

// Helper functions for performing single reads and writes from a block.  These functions are
// provided as a drop in replacement for the discontinued pread and pwrite posix style calls.
// buffer_size and offset are considered sizes in bytes, although reading and writing can only be
// done in whole block increments.  buffer must be pre-allocated to the correct size.  These
// functions are not performant; use the `RemoteBlockDevice` API above if performance matters.
zx_status_t SingleReadBytes(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                            void* buffer, size_t buffer_size, size_t offset);
zx_status_t SingleWriteBytes(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device,
                             void* buffer, size_t buffer_size, size_t offset);

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_REMOTE_BLOCK_DEVICE_H_
