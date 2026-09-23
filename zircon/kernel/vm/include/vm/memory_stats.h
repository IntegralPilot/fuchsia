// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_MEMORY_STATS_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_MEMORY_STATS_H_

#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#define LOCAL_TRACE 0

template <typename T>
T VmoInfoToVersion(const zx_info_vmo_t& vmo);

template <>
inline zx_info_vmo_t VmoInfoToVersion(const zx_info_vmo_t& vmo) {
  return vmo;
}

template <>
zx_info_vmo_v1_t VmoInfoToVersion(const zx_info_vmo& vmo);

template <>
zx_info_vmo_v2_t VmoInfoToVersion(const zx_info_vmo& vmo);

template <>
zx_info_vmo_v3_t VmoInfoToVersion(const zx_info_vmo& vmo);

template <typename T>
T MapsInfoToVersion(const zx_info_maps_t& maps);

template <>
inline zx_info_maps_t MapsInfoToVersion(const zx_info_maps_t& maps) {
  return maps;
}

// template <>
zx_info_maps_v1_t MapsInfoToVersion1(const zx_info_maps_t& maps);

// template <>
zx_info_maps_v2_t MapsInfoToVersion2(const zx_info_maps_t& maps);

template <typename T>
T KernelStatsInfoToVersion(const zx_info_kmem_stats_t& stats);

template <>
zx_info_kmem_stats_v1 KernelStatsInfoToVersion(const zx_info_kmem_stats_t& stats);

template <>
zx_info_kmem_stats_extended_t KernelStatsInfoToVersion(const zx_info_kmem_stats_t& stats);

zx_info_kmem_stats_t GetMemoryStats();
zx_info_kmem_stats_compression_t GetCompressionStats();
zx_info_memory_stall_t GetStallStats();

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_MEMORY_STATS_H_
