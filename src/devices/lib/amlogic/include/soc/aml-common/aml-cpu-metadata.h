// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_COMMON_AML_CPU_METADATA_H_
#define SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_COMMON_AML_CPU_METADATA_H_

#include <lib/ddk/metadata.h>
#include <zircon/types.h>

namespace amlogic_cpu {

// Note that this is only used for Sherlock's proxy driver and should be removed once that
// driver is fully deprecated.
#define DEVICE_METADATA_CLUSTER_SIZE_LEGACY (0x544e4300 | DEVICE_METADATA_PRIVATE)  // CNT

using PerfDomainId = uint32_t;

// Note that this is only used for Sherlock's proxy driver and should be removed once that
// driver is fully deprecated.
typedef struct legacy_cluster_size {
  PerfDomainId pd_id;
  uint32_t core_count;
  uint8_t relative_performance;
} legacy_cluster_info_t;

}  // namespace amlogic_cpu

#endif  // SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_COMMON_AML_CPU_METADATA_H_
