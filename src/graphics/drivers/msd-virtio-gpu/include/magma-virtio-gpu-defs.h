// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MAGMA_VIRTIO_GPU_DEFS_H
#define MAGMA_VIRTIO_GPU_DEFS_H

#include <lib/magma/magma_common_defs.h>

#define MAGMA_VENDOR_VERSION_VIRTIO 1

enum MagmaVirtioGpuQuery {
  // Bits 32..47 indicate the capset id (from virtio spec), bits 48..63 indicate the version.
  // Returns buffer result.
  kMagmaVirtioGpuQueryCapset = MAGMA_QUERY_VENDOR_PARAM_0 + 10000,
};

#endif  // MAGMA_VIRTIO_GPU_DEFS_H
