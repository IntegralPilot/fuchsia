// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FILE_LOCK_FILE_LOCK_H_
#define LIB_FILE_LOCK_FILE_LOCK_H_

#include <lib/fit/function.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <map>
#include <mutex>
#include <set>

namespace file_lock {

using lock_completer_t = fit::callback<void(zx_status_t status)>;

enum LockType {
  READ,
  WRITE,
  UNLOCK,
};

class LockRequest final {
 public:
  LockRequest(LockType type, bool wait) : type_(type), wait_(wait) {}
  bool wait() const { return wait_; }
  LockType type() const { return type_; }

 private:
  LockType type_;
  bool wait_;
};

class FileLock final {
 public:
  FileLock() = default;
  ~FileLock();

  void Lock(zx_koid_t owner, LockRequest& req, lock_completer_t& completer)
      __TA_EXCLUDES(lock_mtx_);
  void Forget(zx_koid_t owner) __TA_EXCLUDES(lock_mtx_);
  bool NoLocksHeld() __TA_EXCLUDES(lock_mtx_);

 private:
  std::mutex lock_mtx_;

  std::map<zx_koid_t, lock_completer_t> pending_shared_ __TA_GUARDED(lock_mtx_);
  std::map<zx_koid_t, lock_completer_t> pending_exclusive_ __TA_GUARDED(lock_mtx_);

  // shared lock <= shared.size() > 0
  // exclusive lock <= exclusive_ != ZX_KOID_INVALID
  std::set<zx_koid_t> shared_ __TA_GUARDED(lock_mtx_);
  zx_koid_t exclusive_ __TA_GUARDED(lock_mtx_) = ZX_KOID_INVALID;

  void LockLocked(zx_koid_t owner, LockRequest& req, lock_completer_t& completer)
      __TA_REQUIRES(lock_mtx_);
};

}  // namespace file_lock

#endif  // LIB_FILE_LOCK_FILE_LOCK_H_
