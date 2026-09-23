// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_SHARED_MUTEX_H_
#define SRC_STORAGE_LIB_VFS_CPP_SHARED_MUTEX_H_

#include <zircon/compiler.h>

#include <shared_mutex>

namespace fs {

// Drop-in replacement for std::shared_lock that has thread safety annotations. The current libcxx
// implementation does not have these annotations, and thus don't work correctly.
//
// TODO(https://fxbug.dev/361092091): this can be removed replaced with std::shared_lock if we
// update to an implementation that is annotated with the correct thread capabilities.
class __TA_SCOPED_CAPABILITY SharedLock {
 public:
  __WARN_UNUSED_CONSTRUCTOR explicit SharedLock(std::shared_mutex& m) __TA_ACQUIRE_SHARED(m)
      : lock_(m) {}
  // It seems like this should be __TA_RELEASE_SHARED instead of __TA_RELEASE but
  // __TA_RELEASE_SHARED gives errors when a ScopedLock goes out of scope:
  //
  //   releasing mutex using shared access, expected exclusive access
  //
  // This is probably a compiler bug.
  ~SharedLock() __TA_RELEASE() {}

  void lock() __TA_ACQUIRE_SHARED() { lock_.lock(); }
  // Same comment about __TA_RELEASE vs. __TA_RELEASE_SHARED as in the destructor.
  void unlock() __TA_RELEASE() { lock_.unlock(); }

 private:
  std::shared_lock<std::shared_mutex> lock_;
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_SHARED_MUTEX_H_
