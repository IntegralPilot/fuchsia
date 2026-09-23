// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/cpu.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_CPU_H_
#define LIB_ZBI_FORMAT_CPU_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

#define ZBI_MAX_SMT ((uint64_t)(4u))

typedef uint16_t zbi_topology_processor_flags_t;

// The associated processor boots the system and is the last to be shutdown.
#define ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY ((zbi_topology_processor_flags_t)(1u << 0))

// The associated processor handles all interrupts. Some architectures
// will not have such a processor.
#define ZBI_TOPOLOGY_PROCESSOR_FLAGS_INTERRUPT ((zbi_topology_processor_flags_t)(1u << 1))

typedef struct {
  // Cluster ids for each level, one being closest to the cpu.
  // These map to aff1, aff2, and aff3 values in the ARM registers.
  uint8_t cluster_1_id;
  uint8_t cluster_2_id;
  uint8_t cluster_3_id;

  // Id of the cpu inside of the bottom-most cluster, aff0 value.
  uint8_t cpu_id;

  // The GIC interface number for this processor.
  // In GIC v3+ this is not necessary as the processors are addressed by their
  // affinity routing (all cluster ids followed by cpu_id).
  uint8_t gic_id;
} zbi_topology_arm64_info_t;

typedef struct {
  uint32_t apic_ids[4];
  uint32_t apic_id_count;
} zbi_topology_x64_info_t;

typedef struct {
  // ID that represents this logical CPU (i.e., hart) in SBI.
  uint64_t hart_id;

  // Index into the ZBI_TYPE_RISCV64_ISA_STRTAB string table payload giving
  // the start of the associated ISA string.
  uint32_t isa_strtab_index;
  uint32_t reserved;
} zbi_topology_riscv64_info_t;

#define ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64 ((uint64_t)(1u))
#define ZBI_TOPOLOGY_ARCHITECTURE_INFO_X64 ((uint64_t)(2u))
#define ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64 ((uint64_t)(3u))

typedef struct {
  uint64_t discriminant;
  union {
    zbi_topology_arm64_info_t arm64;
    zbi_topology_x64_info_t x64;
    zbi_topology_riscv64_info_t riscv64;
  };
} zbi_topology_architecture_info_t;

typedef struct {
  zbi_topology_architecture_info_t architecture_info;
  zbi_topology_processor_flags_t flags;
  uint16_t logical_ids[4];
  uint8_t logical_id_count;
} zbi_topology_processor_t;

typedef struct {
  // Relative performance level of this processor in the system. The value is
  // interpreted as the performance of this processor relative to the maximum
  // performance processor in the system. No specific values are required for
  // the performance level, only that the following relationship holds:
  //
  //   Pmax is the value of performance_class for the maximum performance
  //   processor in the system, operating at its maximum operating point.
  //
  //   P is the value of performance_class for this processor, operating at
  //   its maximum operating point.
  //
  //   R is the performance ratio of this processor to the maximum performance
  //   processor in the system in the range (0.0, 1.0].
  //
  //   R = (P + 1) / (Pmax + 1)
  //
  // If accuracy is limited, choose a conservative value that slightly under-
  // estimates the performance of lower-performance processors.
  uint8_t performance_class;
} zbi_topology_cluster_t;

typedef struct {
  // Unique id of this cache node. No other semantics are assumed.
  uint32_t cache_id;
} zbi_topology_cache_t;

typedef struct {
  uint64_t reserved;
} zbi_topology_die_t;

typedef struct {
  uint64_t reserved;
} zbi_topology_socket_t;

typedef struct {
  // Starting memory addresses of the numa region.
  uint64_t start;

  // Size in bytes of the numa region.
  uint64_t size;
} zbi_topology_numa_region_t;

#define ZBI_TOPOLOGY_ENTITY_PROCESSOR ((uint64_t)(1u))
#define ZBI_TOPOLOGY_ENTITY_CLUSTER ((uint64_t)(2u))
#define ZBI_TOPOLOGY_ENTITY_CACHE ((uint64_t)(3u))
#define ZBI_TOPOLOGY_ENTITY_DIE ((uint64_t)(4u))
#define ZBI_TOPOLOGY_ENTITY_SOCKET ((uint64_t)(5u))
#define ZBI_TOPOLOGY_ENTITY_NUMA_REGION ((uint64_t)(6u))

typedef struct {
  uint64_t discriminant;
  union {
    zbi_topology_processor_t processor;
    zbi_topology_cluster_t cluster;
    zbi_topology_cache_t cache;
    zbi_topology_die_t die;
    zbi_topology_socket_t socket;
    zbi_topology_numa_region_t numa_region;
  };
} zbi_topology_entity_t;

#define ZBI_TOPOLOGY_NO_PARENT ((uint16_t)(0xffffu))

// The ZBI_TYPE_CPU_TOPOLOGY payload consists of an array of
// zbi_topology_node_t, giving a flattened tree-like description of the CPU
// configuration according to the entity hierarchy.
typedef struct {
  zbi_topology_entity_t entity;
  uint16_t parent_index;
} zbi_topology_node_t;

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_CPU_H_
