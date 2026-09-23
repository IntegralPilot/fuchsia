// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/connection/advisory_lock.h"

#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/file-lock/file-lock.h>
#include <lib/fit/function.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/ref_ptr.h>

#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs::internal {

void advisory_lock(zx_koid_t owner, fbl::RefPtr<fs::Vnode> vnode, bool range_ok,
                   ::fuchsia_io::wire::AdvisoryLockRequest& request,
                   fit::callback<void(zx_status_t status)> callback) {
  if (owner == ZX_KOID_INVALID) {
    callback(ZX_ERR_INTERNAL);
    return;
  }
  if (!request.has_type()) {
    callback(ZX_ERR_INVALID_ARGS);
    return;
  }
  file_lock::LockType lock_type;
  switch (request.type()) {
    case fuchsia_io::wire::AdvisoryLockType::kRead:
      lock_type = file_lock::LockType::READ;
      break;
    case fuchsia_io::wire::AdvisoryLockType::kWrite:
      lock_type = file_lock::LockType::WRITE;
      break;
    case fuchsia_io::wire::AdvisoryLockType::kUnlock:
      lock_type = file_lock::LockType::UNLOCK;
      break;
  }

  // TODO(https://fxbug.dev/42150671) implement range locking
  // for F_SETLK, F_SETLKW, and F_GETLK fcntl operations
  if (request.has_range()) {
    callback(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  auto lock = vnode->GetVnodeFileLock();
  if (lock == nullptr) {
    callback(ZX_ERR_INTERNAL);
    return;
  }
  bool wait = request.has_wait() && request.wait();
  file_lock::LockRequest req(lock_type, wait);
  lock->Lock(owner, req, callback);
}

}  // namespace fs::internal
