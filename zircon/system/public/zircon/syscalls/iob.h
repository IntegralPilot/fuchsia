// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSCALLS_IOB_H_
#define ZIRCON_SYSCALLS_IOB_H_

#include <stdint.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

// A somewhat arbitrary limitation on the number of IOBuffer regions to protect
// us from having to handle IOBuffers with extremely large numbers of regions.
#define ZX_IOB_MAX_REGIONS 64

// The type of IOBuffer discipline.
typedef uint64_t zx_iob_discipline_type_t;

// No particular discipline indicates that the region is treated as a raw byte
// buffer from the kernel's perspective, the organization of its contents being
// a private userspace matter.
#define ZX_IOB_DISCIPLINE_TYPE_NONE ((zx_iob_discipline_type_t)(0u))

// TODO(https://fxbug.dev/319501447): + ID allocator discipline.
// TODO(https://fxbug.dev/319500512): + ring buffer discipline.

// An IOBuffer (memory access) discipline specifies the layout of a region's
// memory and manner in which it should be directly accessed. Each discipline
// defines a container of sorts backed by the region's memory. These container
// interfaces are also mirrored in discipline-specific syscalls that permit
// indirect, kernel-mediated access to the region in this manner.
typedef struct zx_iob_discipline {
  zx_iob_discipline_type_t type;
  union {
    uint64_t reserved[8];
  };
} zx_iob_discipline_t;

// The type of a region specifies the nature of its backing memory object and
// its ownership semantics.
typedef uint32_t zx_iob_region_type_t;

// Specifies a region backed by a private memory object uniquely owned and
// accessible by the associated IOBuffer.
#define ZX_IOB_REGION_TYPE_PRIVATE ((zx_iob_region_type_t)(0u))

// Region access controls for IOBuffer peers, specifying both mapped and
// mediated access.
//
// "Mediated access" does not refer to access of the underlying region memory,
// but rather access through the interface of the associated discipline
// *container* backed by that memory, which will admit its own notions of
// reading and writing of *entries*.
typedef uint32_t zx_iob_access_t;

#define ZX_IOB_ACCESS_EP0_CAN_MAP_READ ((zx_iob_access_t)(1u << 0))
#define ZX_IOB_ACCESS_EP0_CAN_MAP_WRITE ((zx_iob_access_t)(1u << 1))
#define ZX_IOB_ACCESS_EP0_CAN_MEDIATED_READ ((zx_iob_access_t)(1u << 2))
#define ZX_IOB_ACCESS_EP0_CAN_MEDIATED_WRITE ((zx_iob_access_t)(1u << 3))
#define ZX_IOB_ACCESS_EP1_CAN_MAP_READ ((zx_iob_access_t)(1u << 4))
#define ZX_IOB_ACCESS_EP1_CAN_MAP_WRITE ((zx_iob_access_t)(1u << 5))
#define ZX_IOB_ACCESS_EP1_CAN_MEDIATED_READ ((zx_iob_access_t)(1u << 6))
#define ZX_IOB_ACCESS_EP1_CAN_MEDIATED_WRITE ((zx_iob_access_t)(1u << 7))

typedef struct zx_iob_region_private {
  uint32_t options;
  uint32_t padding1;
  uint64_t padding2[3];
} zx_iob_region_private_t;

typedef struct zx_iob_region {
  zx_iob_region_type_t type;
  zx_iob_access_t access;
  uint64_t size;
  zx_iob_discipline_t discipline;
  union {
    zx_iob_region_private_t private_region;
    uint8_t max_extension[32];
  };
} zx_iob_region_t;

// Options for `zx_iob_write()`.
typedef uint64_t zx_iob_write_options_t;

__END_CDECLS

#endif  // ZIRCON_SYSCALLS_IOB_H_
