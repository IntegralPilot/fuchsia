// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/fvm/client.h"

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <stdlib.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <iterator>

#include "src/storage/lib/block_client/cpp/block_device.h"

namespace fvm {

zx_status_t ResetAllSlices(block_client::BlockDevice* device) {
  uint64_t vslice_start[1];
  vslice_start[0] = 1;

  while (true) {
    fuchsia_storage_block::wire::VsliceRange ranges[fuchsia_storage_block::wire::kMaxSliceRequests];
    size_t actual_ranges_count;
    zx_status_t status = device->VolumeQuerySlices(vslice_start, std::size(vslice_start), ranges,
                                                   &actual_ranges_count);
    if (status == ZX_ERR_OUT_OF_RANGE) {
      return ZX_OK;
    }
    if (status != ZX_OK) {
      return status;
    }
    if (actual_ranges_count != 1 || ranges[0].count == 0) {
      return ZX_ERR_IO;
    }

    // Free any slices that were allocated.
    if (ranges[0].allocated) {
      status = device->VolumeShrink(vslice_start[0], ranges[0].count);
      if (status != ZX_OK) {
        return status;
      }
    }

    // Move to the next portion of the block address space.
    vslice_start[0] += ranges[0].count;
  }
}

}  // namespace fvm
