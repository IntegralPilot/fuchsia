// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/memory.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_MEMORY_H_
#define LIB_ZBI_FORMAT_MEMORY_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

// Unknown values should be treated as RESERVED to allow forwards compatibility.
typedef uint32_t zbi_mem_type_t;

// Standard RAM.
#define ZBI_MEM_TYPE_RAM ((zbi_mem_type_t)(1u))

// Device memory.
#define ZBI_MEM_TYPE_PERIPHERAL ((zbi_mem_type_t)(2u))

// Represents memory that should not be used by the system. Reserved ranges may
// overlap other RAM or PERIPHERAL regions, in which case the reserved range
// should take precedence.
#define ZBI_MEM_TYPE_RESERVED ((zbi_mem_type_t)(3u))

// The ZBI_TYPE_MEM_CONFIG payload consist of one or more `zbi_mem_range_t`
// entries.
//
// The length of the item is `sizeof(zbi_mem_range_t)` times the number of
// entries. Each entry describes a contiguous range of memory
//
// Entries in the table may be in any order, and only a single item of type
// ZBI_TYPE_MEM_CONFIG should be present in the ZBI.
typedef struct {
  uint64_t paddr;
  uint64_t length;
  zbi_mem_type_t type;
  uint32_t reserved;
} zbi_mem_range_t;

// ZBI_TYPE_NVRAM payload.
typedef struct {
  uint64_t base;
  uint64_t length;
} zbi_nvram_t;

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_MEMORY_H_
