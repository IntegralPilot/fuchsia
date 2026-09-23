// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_INCLUDE_LIB_MAGMA_SERVICE_MSD_DEFS_H_
#define SRC_GRAPHICS_MAGMA_INCLUDE_LIB_MAGMA_SERVICE_MSD_DEFS_H_

#include <lib/magma/magma_common_defs.h>
#include <stdint.h>

#include <string>

namespace msd {

#define MSD_DRIVER_CONFIG_TEST_NO_DEVICE_THREAD 1
// Designed so that msd_notification_t fits in a page
#define MSD_CHANNEL_SEND_MAX_SIZE (4096 - sizeof(uint64_t) - sizeof(uint32_t))

typedef uint64_t msd_client_id_t;

enum IcdSupportFlags : uint32_t {
  ICD_SUPPORT_FLAG_VULKAN = 1,
  ICD_SUPPORT_FLAG_OPENCL = 2,
  ICD_SUPPORT_FLAG_MEDIA_CODEC_FACTORY = 4,
};

struct MsdIcdInfo {
  std::string component_url;
  IcdSupportFlags support_flags;
};

enum MagmaMemoryPressureLevel {
  MAGMA_MEMORY_PRESSURE_LEVEL_NORMAL = 1,
  MAGMA_MEMORY_PRESSURE_LEVEL_WARNING = 2,
  MAGMA_MEMORY_PRESSURE_LEVEL_CRITICAL = 3,
};

// A batch buffer to be executed plus the resources required to execute it
struct magma_command_buffer {
  uint32_t resource_count;
  uint32_t batch_buffer_resource_index;  // resource index of the batch buffer to execute
  uint64_t batch_start_offset;           // relative to the starting offset of the buffer
  uint32_t wait_semaphore_count;
  uint32_t signal_semaphore_count;
  uint64_t flags;
};

}  // namespace msd

#endif /* _MSD_DEFS_H_ */
