// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "file-lock.h"

#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <map>
#include <mutex>
#include <utility>

namespace file_lock {

FileLock::~FileLock() {
  const std::scoped_lock lock(lock_mtx_);
  for (auto& client : pending_shared_) {
    client.second(ZX_ERR_CANCELED);
  }
  for (auto& client : pending_exclusive_) {
    client.second(ZX_ERR_CANCELED);
  }
}

void FileLock::Lock(zx_koid_t owner, LockRequest& req, lock_completer_t& completer) {
  const std::scoped_lock lock(lock_mtx_);
  LockLocked(owner, req, completer);
}

void FileLock::LockLocked(zx_koid_t owner, LockRequest& req, lock_completer_t& completer) {
  if (pending_exclusive_.find(owner) != pending_exclusive_.end() ||
      pending_shared_.find(owner) != pending_shared_.end()) {
    completer(ZX_ERR_BAD_STATE);
    return;
  }

  if (exclusive_ != ZX_KOID_INVALID) {
    if (owner == exclusive_) {
      switch (req.type()) {
        case LockType::READ: {
          // downgrading from exclusive to shared
          // unblock all the shared clients
          exclusive_ = ZX_KOID_INVALID;
          shared_.emplace(owner);
          std::map ps(std::move(pending_shared_));
          for (auto& owner_info : ps) {
            shared_.emplace(owner_info.first);
          }
          for (auto& owner_info : ps) {
            owner_info.second(ZX_OK);
          }
          completer(ZX_OK);
          break;
        }
        case LockType::WRITE: {
          // already have this - noop
          completer(ZX_OK);
          break;
        }
        case LockType::UNLOCK: {
          exclusive_ = ZX_KOID_INVALID;
          // this appears to be the logic of flock
          if (pending_shared_.size() > pending_exclusive_.size()) {
            std::map ps(std::move(pending_shared_));
            for (auto& owner_info : ps) {
              shared_.emplace(owner_info.first);
            }
            for (auto& owner_info : ps) {
              owner_info.second(ZX_OK);
            }
          } else {
            auto first_exclusive = pending_exclusive_.begin();
            if (first_exclusive != pending_exclusive_.end()) {
              exclusive_ = first_exclusive->first;
              auto exclusive_completer(std::move(first_exclusive->second));
              pending_exclusive_.erase(first_exclusive);
              exclusive_completer(ZX_OK);
            }
          }
          completer(ZX_OK);
          break;
        }
      }
    } else {
      switch (req.type()) {
        case LockType::READ: {
          if (req.wait()) {
            pending_shared_.emplace(owner, std::move(completer));
          } else {
            completer(ZX_ERR_SHOULD_WAIT);
          }
          break;
        }
        case LockType::WRITE: {
          if (req.wait()) {
            pending_exclusive_.emplace(owner, std::move(completer));
          } else {
            completer(ZX_ERR_SHOULD_WAIT);
          }
          break;
        }
        case LockType::UNLOCK: {
          // did not have a lock - noop
          completer(ZX_OK);
          break;
        }
      }
    }
  } else {
    switch (req.type()) {
      case LockType::READ: {
        shared_.emplace(owner);
        completer(ZX_OK);
        break;
      }
      case LockType::WRITE: {
        shared_.erase(owner);
        if (shared_.empty()) {
          exclusive_ = owner;
          completer(ZX_OK);
        } else {
          if (req.wait()) {
            pending_exclusive_.emplace(owner, std::move(completer));
          } else {
            // if we had a lock, we lost it
            completer(ZX_ERR_SHOULD_WAIT);
          }
        }
        break;
      }
      case LockType::UNLOCK: {
        auto extracted = shared_.extract(owner);
        if (extracted.empty()) {
          // did not have a lock - noop
          completer(ZX_OK);
        } else {
          auto first_exclusive = pending_exclusive_.begin();
          if (first_exclusive != pending_exclusive_.end() && shared_.empty()) {
            exclusive_ = first_exclusive->first;
            auto exclusive_completer(std::move(first_exclusive->second));
            pending_exclusive_.erase(first_exclusive);
            exclusive_completer(ZX_OK);
          }
          completer(ZX_OK);
        }
        break;
      }
    }
  }
}

void FileLock::Forget(zx_koid_t owner) {
  std::scoped_lock lock(lock_mtx_);
  if (exclusive_ == owner || shared_.find(owner) != shared_.end()) {
    LockRequest req(LockType::UNLOCK, false);
    lock_completer_t completer([](zx_status_t status) { ZX_DEBUG_ASSERT(status == ZX_OK); });
    LockLocked(owner, req, completer);
  }
  if (auto node = pending_exclusive_.extract(owner); !node.empty()) {
    node.mapped()(ZX_ERR_CANCELED);
  }
  if (auto node = pending_shared_.extract(owner); !node.empty()) {
    node.mapped()(ZX_ERR_CANCELED);
  }
}

bool FileLock::NoLocksHeld() {
  std::scoped_lock lock(lock_mtx_);
  return exclusive_ == ZX_KOID_INVALID && shared_.empty() && pending_exclusive_.empty() &&
         pending_shared_.empty();
}

}  // namespace file_lock
