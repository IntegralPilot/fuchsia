// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_OPERATION_TRACKER_STUB_H_
#define SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_OPERATION_TRACKER_STUB_H_

#include <lib/zx/time.h>
#include <zircon/types.h>

#include "src/storage/lib/vfs/cpp/inspect/operation_tracker/operation_tracker_base.h"

namespace fs_inspect {

// Stub implementation of OperationTracker for host builds.
class OperationTrackerStub final : public OperationTracker {
 public:
  OperationTrackerStub() = default;

 private:
  void OnSuccess(zx::duration latency) override {}
  void OnError(zx_status_t error) override {}
  void OnError() override {}
};

}  // namespace fs_inspect

#endif  // SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_OPERATION_TRACKER_STUB_H_
