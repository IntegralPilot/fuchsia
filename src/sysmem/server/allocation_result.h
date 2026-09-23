// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_ALLOCATION_RESULT_H_
#define SRC_SYSMEM_SERVER_ALLOCATION_RESULT_H_

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <zircon/types.h>

namespace sysmem_service {

struct AllocationResult {
  const fuchsia_sysmem2::BufferCollectionInfo* buffer_collection_info = nullptr;
  const std::optional<fuchsia_sysmem2::Error> maybe_error;
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_ALLOCATION_RESULT_H_
