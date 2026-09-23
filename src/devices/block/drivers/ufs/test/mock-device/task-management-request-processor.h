// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_TASK_MANAGEMENT_REQUEST_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_TASK_MANAGEMENT_REQUEST_PROCESSOR_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/mmio-ptr/fake.h>

#include <functional>
#include <vector>

#include "handler.h"
#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {
namespace ufs_mock_device {

class UfsMockDevice;

class TaskManagementRequestProcessor {
 public:
  using TaskManagementRequestHandler =
      std::function<zx_status_t(UfsMockDevice &, TaskManagementRequestDescriptor &)>;

  TaskManagementRequestProcessor(const TaskManagementRequestProcessor &) = delete;
  TaskManagementRequestProcessor &operator=(const TaskManagementRequestProcessor &) = delete;
  TaskManagementRequestProcessor(const TaskManagementRequestProcessor &&) = delete;
  TaskManagementRequestProcessor &operator=(const TaskManagementRequestProcessor &&) = delete;
  ~TaskManagementRequestProcessor() = default;
  explicit TaskManagementRequestProcessor(UfsMockDevice &mock_device) : mock_device_(mock_device) {}
  void HandleTaskManagementRequest(TaskManagementRequestDescriptor &descriptor);

  static zx_status_t DefaultAbortTaskHandler(UfsMockDevice &mock_device,
                                             TaskManagementRequestDescriptor &descriptor);

  static zx_status_t DefaultQueryTaskHandler(UfsMockDevice &mock_device,
                                             TaskManagementRequestDescriptor &descriptor);

  static zx_status_t DefaultLogicalUnitResetHandler(UfsMockDevice &mock_device,
                                                    TaskManagementRequestDescriptor &descriptor);

  DEF_DEFAULT_HANDLER_BEGIN(TaskManagementFunction, TaskManagementRequestHandler)
  DEF_DEFAULT_HANDLER(TaskManagementFunction::kAbortTask, DefaultAbortTaskHandler)
  DEF_DEFAULT_HANDLER(TaskManagementFunction::kQueryTask, DefaultQueryTaskHandler)
  DEF_DEFAULT_HANDLER(TaskManagementFunction::kLogicalUnitReset, DefaultLogicalUnitResetHandler)
  DEF_DEFAULT_HANDLER_END()

 private:
  UfsMockDevice &mock_device_;
};

}  // namespace ufs_mock_device
}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_TASK_MANAGEMENT_REQUEST_PROCESSOR_H_
