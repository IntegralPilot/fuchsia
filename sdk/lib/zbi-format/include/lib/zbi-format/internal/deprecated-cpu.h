// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZBI_FORMAT_INTERNAL_DEPRECATED_CPU_H_
#define LIB_ZBI_FORMAT_INTERNAL_DEPRECATED_CPU_H_

#include <lib/zbi-format/cpu.h>
#include <lib/zbi-format/zbi.h>
#include <stdint.h>

// Legacy v1 CPU configuration. See zbi_cpu_config_t for a description of the payload.
//
// 'CPUC'
#define ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 ((zbi_type_t)(0x43555043))

// Legacy v2 CPU configuration. See zbi_topology_node_v2_t for a description of the payload.
//
// 'TOPO'
#define ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2 ((zbi_type_t)(0x544f504f))

typedef struct {
  // Number of CPU cores in the cluster.
  uint32_t cpu_count;

  // Reserved for future use.  Set to 0.
  uint32_t type;
  uint32_t flags;
  uint32_t reserved;
} zbi_cpu_cluster_t;

// ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 payload.
//
// zbi_header_t.length must equal
// ```
// zbi_cpu_config_t.cluster_count * sizeof(zbi_cpu_cluster_t)
// ```
//
// This item type has been deprecated in favour of ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2.
typedef struct {
  // Number of zbi_cpu_cluster_t entries following this header.
  uint32_t cluster_count;

  // Reserved for future use.  Set to 0.
  uint32_t reserved[3];

  // cluster_count entries follow.
  zbi_cpu_cluster_t clusters[];
} zbi_cpu_config_t;

typedef uint8_t zbi_topology_architecture_v2_t;

// Intended primarily for testing.
#define ZBI_TOPOLOGY_ARCHITECTURE_V2_UNDEFINED ((zbi_topology_architecture_v2_t)(0u))
#define ZBI_TOPOLOGY_ARCHITECTURE_V2_X64 ((zbi_topology_architecture_v2_t)(1u))
#define ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64 ((zbi_topology_architecture_v2_t)(2u))
#define ZBI_TOPOLOGY_ARCHITECTURE_V2_RISCV64 ((zbi_topology_architecture_v2_t)(3u))

typedef struct {
  uint16_t logical_ids[ZBI_MAX_SMT];
  uint8_t logical_id_count;

  zbi_topology_processor_flags_t flags;

  // If UNDEFINED then nothing will be set in arch_info.
  zbi_topology_architecture_v2_t architecture;
  union {
    zbi_topology_arm64_info_t arm64;
    zbi_topology_x64_info_t x64;
    zbi_topology_riscv64_info_t riscv64;
  } architecture_info;

} zbi_topology_processor_v2_t;

typedef struct {
  // Starting and ending memory addreses of this numa region.
  uint64_t start_address;
  uint64_t end_address;
} zbi_topology_numa_region_v2_t;

typedef uint8_t zbi_topology_entity_type_v2_t;

// Unused default.
#define ZBI_TOPOLOGY_ENTITY_V2_UNDEFINED ((zbi_topology_entity_type_v2_t)(0u))
#define ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR ((zbi_topology_entity_type_v2_t)(1u))
#define ZBI_TOPOLOGY_ENTITY_V2_CLUSTER ((zbi_topology_entity_type_v2_t)(2u))
#define ZBI_TOPOLOGY_ENTITY_V2_CACHE ((zbi_topology_entity_type_v2_t)(3u))
#define ZBI_TOPOLOGY_ENTITY_V2_DIE ((zbi_topology_entity_type_v2_t)(4u))
#define ZBI_TOPOLOGY_ENTITY_V2_SOCKET ((zbi_topology_entity_type_v2_t)(5u))
#define ZBI_TOPOLOGY_ENTITY_V2_POWER_PLANE ((zbi_topology_entity_type_v2_t)(6u))
#define ZBI_TOPOLOGY_ENTITY_V2_NUMA_REGION ((zbi_topology_entity_type_v2_t)(7u))

// The ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2 consists of an array of zbi_topology_node_v2_t,
// giving a flattened tree-like description of the CPU configuration
// according to the zbi_topology_entity_type_v2_t hierarchy.
typedef struct {
  zbi_topology_entity_type_v2_t entity_type;
  uint16_t parent_index;
  union {
    zbi_topology_processor_v2_t processor;
    zbi_topology_cluster_t cluster;
    zbi_topology_numa_region_v2_t numa_region;
    zbi_topology_cache_t cache;
  } entity;
} zbi_topology_node_v2_t;

#endif  // LIB_ZBI_FORMAT_INTERNAL_DEPRECATED_CPU_H_
