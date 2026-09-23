// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_H_
#define SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_H_

//
// Provides tracking of various filesystem operations, including stubs for host builds.
//

#include "src/storage/lib/vfs/cpp/inspect/operation_tracker/operation_tracker_base.h"  // IWYU pragma: export

#ifdef __Fuchsia__
#include "src/storage/lib/vfs/cpp/inspect/operation_tracker/operation_tracker_fuchsia.h"  // IWYU pragma: export
#else
#include "src/storage/lib/vfs/cpp/inspect/operation_tracker/operation_tracker_stub.h"  // IWYU pragma: export
#endif
namespace fs_inspect::internal {
#ifdef __Fuchsia__
using OperationTrackerType = OperationTrackerFuchsia;
#else
using OperationTrackerType = OperationTrackerStub;
#endif
}  // namespace fs_inspect::internal

#endif  // SRC_STORAGE_LIB_VFS_CPP_INSPECT_OPERATION_TRACKER_H_
