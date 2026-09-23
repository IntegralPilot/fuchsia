// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_MISC_DRIVERS_VIRTIO_PMEM_VIRTIO_PMEM_H_
#define SRC_DEVICES_MISC_DRIVERS_VIRTIO_PMEM_VIRTIO_PMEM_H_

#include <stdint.h>

// Virtio 5.19 PMEM Device

// 5.19.3 Feature bits
// The guest physical address range will be indicated as a shared memory region.
#define VIRTIO_PMEM_F_SHMEM_REGION ((uint64_t)1 << 0)

// 5.19.4 Device configuration layout
struct virtio_pmem_config {
  uint64_t start;
  uint64_t size;
};

// 5.19.6 Driver Operations
struct virtio_pmem_req {
  int32_t type;
};

#define VIRTIO_PMEM_REQ_TYPE_FLUSH ((int32_t)0)

// 5.19.7.2 Device operations
struct virtio_pmem_resp {
  int32_t ret;  // 0 means success, -1 means failure.
};

#endif  // SRC_DEVICES_MISC_DRIVERS_VIRTIO_PMEM_VIRTIO_PMEM_H_
