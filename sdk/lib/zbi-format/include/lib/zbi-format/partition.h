// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/partition.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_PARTITION_H_
#define LIB_ZBI_FORMAT_PARTITION_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

#define ZBI_PARTITION_NAME_LEN ((uint64_t)(32u))

#define ZBI_PARTITION_GUID_LEN ((uint64_t)(16u))

typedef uint8_t zbi_partition_guid_t[16];

typedef struct {
  // GUID specifying the format and use of data stored in the partition.
  zbi_partition_guid_t type_guid;

  // GUID unique to this partition.
  zbi_partition_guid_t uniq_guid;

  // First and last block occupied by this partition.
  uint64_t first_block;
  uint64_t last_block;

  // Reserved for future use.  Set to 0.
  uint64_t flags;
  char name[32];
} zbi_partition_t;

// ZBI_TYPE_DRV_PARTITION_MAP payload. This header is immediately followed by
// an array of the corresponding zbi_partition_t.
typedef struct {
  // Total blocks used on the device.
  uint64_t block_count;

  // Size of each block in bytes.
  uint64_t block_size;

  // Number of partitions in the map.
  uint32_t partition_count;

  // Reserved for future use.
  uint32_t reserved;

  // Device GUID.
  zbi_partition_guid_t guid;
} zbi_partition_map_t;

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_PARTITION_H_
