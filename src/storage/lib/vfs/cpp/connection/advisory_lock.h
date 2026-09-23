// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_CONNECTION_ADVISORY_LOCK_H_
#define SRC_STORAGE_LIB_VFS_CPP_CONNECTION_ADVISORY_LOCK_H_

#ifndef __Fuchsia__
#error "Fuchsia-only header"
#endif

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fit/function.h>
#include <zircon/types.h>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs::internal {

void advisory_lock(zx_koid_t owner, fbl::RefPtr<fs::Vnode> vnode, bool range_ok,
                   ::fuchsia_io::wire::AdvisoryLockRequest& request,
                   fit::callback<void(zx_status_t status)> callback);

}  // namespace fs::internal

#endif  // SRC_STORAGE_LIB_VFS_CPP_CONNECTION_ADVISORY_LOCK_H_
