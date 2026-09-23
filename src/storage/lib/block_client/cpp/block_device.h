// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_BLOCK_DEVICE_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_BLOCK_DEVICE_H_

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <memory>

#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/vmoid_registry.h"

namespace block_client {

// A high-level interface to a block device. This class also inherits from VmoidRegistry for
// managing the VMOs associated with block requests.
//
// The normal implementation would be a RemoteBlockDevice which speaks the FIDL/FIFO protocols
class BlockDevice : public storage::VmoidRegistry {
 public:
  // FIFO protocol. This is the normal way to read from and write to the block device.
  virtual zx_status_t FifoTransaction(BlockFifoRequest* requests, size_t count) = 0;

  // Returns the full topological path of the device.
  virtual zx::result<std::string> GetTopologicalPath() const = 0;

  // Attempts to rebind a given driver to the device. If `url_suffix` is empty, then the device
  // will be bound based on its bind rules.
  virtual zx::result<> Rebind(std::string_view url_suffix) const = 0;

  // fuchsia.storage.block interface:
  virtual zx_status_t BlockGetInfo(fuchsia_storage_block::wire::BlockInfo* out_info) const = 0;

  // storage::VmoidRegistry implementation:
  //
  // Derived classes will need to implement BlockAttachVmo() according to their requirements. This
  // implementation implements detach by sending a FIFO transaction which should work for most
  // cases.
  zx_status_t BlockDetachVmo(storage::Vmoid vmoid) override;

  // fuchsia.storage.block.Block volume interface:
  //
  // Some block devices (like FVM partitions) are also volumes. This provides a convenience wrapper
  // for speaking the volume-specific methods.
  //
  // If the underlying device does not speak the Volume API, these calls will fail with
  // ZX_ERR_NOT_SUPPORTED.  Clients should call VolumeGetInfo() and check for ZX_OK to confirm that
  // the device supports the Volume API before using any other Volume methods.
  virtual zx_status_t VolumeGetInfo(
      fuchsia_storage_block::wire::VolumeManagerInfo* out_manager_info,
      fuchsia_storage_block::wire::VolumeInfo* out_volume_info) const = 0;
  virtual zx_status_t VolumeQuerySlices(const uint64_t* slices, size_t slices_count,
                                        fuchsia_storage_block::wire::VsliceRange* out_ranges,
                                        size_t* out_ranges_count) const = 0;
  virtual zx_status_t VolumeExtend(uint64_t offset, uint64_t length) = 0;
  virtual zx_status_t VolumeShrink(uint64_t offset, uint64_t length) = 0;
};

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_BLOCK_DEVICE_H_
