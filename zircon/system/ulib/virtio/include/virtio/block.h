// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef VIRTIO_BLOCK_H_
#define VIRTIO_BLOCK_H_

#include <stdint.h>
#include <zircon/compiler.h>

// clang-format off
#define VIRTIO_BLK_F_BARRIER    ((uint64_t)1 << 0)
#define VIRTIO_BLK_F_SIZE_MAX   ((uint64_t)1 << 1)
#define VIRTIO_BLK_F_SEG_MAX    ((uint64_t)1 << 2)
#define VIRTIO_BLK_F_GEOMETRY   ((uint64_t)1 << 4)
#define VIRTIO_BLK_F_RO         ((uint64_t)1 << 5)
#define VIRTIO_BLK_F_BLK_SIZE   ((uint64_t)1 << 6)
#define VIRTIO_BLK_F_SCSI       ((uint64_t)1 << 7)
#define VIRTIO_BLK_F_FLUSH      ((uint64_t)1 << 9)
#define VIRTIO_BLK_F_TOPOLOGY   ((uint64_t)1 << 10)
#define VIRTIO_BLK_F_CONFIG_WCE ((uint64_t)1 << 11)
#define VIRTIO_BLK_F_DISCARD    ((uint64_t)1 << 13)

#define VIRTIO_BLK_T_IN         0
#define VIRTIO_BLK_T_OUT        1
#define VIRTIO_BLK_T_FLUSH      4
#define VIRTIO_BLK_T_GET_ID     8
#define VIRTIO_BLK_T_DISCARD    11
#define VIRTIO_BLK_T_BARRIER    0x80000000

#define VIRTIO_BLK_S_OK         0
#define VIRTIO_BLK_S_IOERR      1
#define VIRTIO_BLK_S_UNSUPP     2

#define VIRTIO_BLK_ID_BYTES     20
// clang-format on

__BEGIN_CDECLS

typedef struct virtio_blk_geometry {
  uint16_t cylinders;
  uint8_t heads;
  uint8_t sectors;
} __PACKED virtio_blk_geometry_t;

typedef struct virtio_blk_config {
  uint64_t capacity;
  uint32_t size_max;
  uint32_t seg_max;
  virtio_blk_geometry_t geometry;
  uint32_t blk_size;
} __PACKED virtio_blk_config_t;

typedef struct virtio_blk_req {
  uint32_t type;
  uint32_t ioprio;
  uint64_t sector;
} __PACKED virtio_blk_req_t;

typedef struct virtio_blk_discard_write_zeroes {
  uint64_t sector;
  uint32_t num_sectors;
  uint32_t flags;
} __PACKED virtio_blk_discard_write_zeroes_t;

__END_CDECLS

#endif  // VIRTIO_BLOCK_H_
