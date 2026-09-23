// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TASK_MANAGEMENT_REQUEST_DESCRIPTOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TASK_MANAGEMENT_REQUEST_DESCRIPTOR_H_

#include <hwreg/bitfields.h>

#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"

namespace ufs {

// UFSHCI Specification Version 3.0, section 6.2.1 "UTP Task Management Request Descriptor".
struct TaskManagementRequestDescriptor {
  uint32_t dwords[20];

  // dword 0
  DEF_SUBBIT(dwords[0], 24, interrupt);
  // dword 2
  DEF_ENUM_SUBFIELD(dwords[2], OverallCommandStatus, 7, 0, overall_command_status);

  TaskManagementRequestUpiuData *GetRequestData() {
    return reinterpret_cast<TaskManagementRequestUpiuData *>(&dwords[4]);
  }
  TaskManagementResponseUpiuData *GetResponseData() {
    return reinterpret_cast<TaskManagementResponseUpiuData *>(&dwords[12]);
  }
} __PACKED;
static_assert(sizeof(TaskManagementRequestDescriptor) == 80,
              "TaskManagementRequestDescriptor struct must be 80 bytes");

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TASK_MANAGEMENT_REQUEST_DESCRIPTOR_H_
