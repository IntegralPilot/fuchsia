// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_SPARSE_H_
#define SRC_FIRMWARE_PAVER_SPARSE_H_

#include <lib/zx/vmo.h>

#include <cstddef>

#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/partition-client.h"

namespace paver {

/// Writes the Android Sparse-formatted image from `payload_vmo` into `partition`.
zx::result<> WriteSparse(PartitionClient& partition, const PartitionSpec& spec, zx::vmo payload_vmo,
                         size_t payload_size);

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_SPARSE_H_
