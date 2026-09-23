// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_DIR_ENTRY_CACHE_H_
#define SRC_STORAGE_F2FS_DIR_ENTRY_CACHE_H_

#include <fbl/ref_counted.h>

#include "src/storage/f2fs/common.h"

namespace f2fs {

constexpr pgoff_t kCachedInlineDirEntryPageIndex = -1;

struct DentryInfo {
  ino_t ino = 0;
  pgoff_t page_index = 0;
  size_t bit_pos = 0;
};

class DirEntryCacheElement : public fbl::RefCounted<DirEntryCacheElement> {
 public:
  DirEntryCacheElement(std::string_view name) : name_(name) {}
  std::string_view GetName() const { return std::string_view(name_); }
  DentryInfo GetInfo() const { return dentry_info_; }
  void SetInfo(DentryInfo dentry_info) { dentry_info_ = dentry_info; }

 private:
  std::string name_;
  DentryInfo dentry_info_;
};

using ElementRefPtr = fbl::RefPtr<DirEntryCacheElement>;
using EntryKey = std::string;

class DirEntryCache {
 public:
  DirEntryCache();

  DirEntryCache(const DirEntryCache &) = delete;
  DirEntryCache &operator=(const DirEntryCache &) = delete;
  DirEntryCache(DirEntryCache &&) = delete;
  DirEntryCache &operator=(DirEntryCache &&) = delete;

  ~DirEntryCache();

  void Reset() __TA_EXCLUDES(lock_);

  zx::result<DentryInfo> LookupDirEntry(std::string_view child_name) __TA_EXCLUDES(lock_);
  void UpdateDirEntry(std::string_view child_name, DentryInfo info) __TA_EXCLUDES(lock_);
  void RemoveDirEntry(std::string_view child_name) __TA_EXCLUDES(lock_);

  // For testing
  bool IsElementInCache(std::string_view child_name) const __TA_EXCLUDES(lock_);
  const std::map<EntryKey, ElementRefPtr> &GetMap() const __TA_EXCLUDES(lock_);

 private:
  DirEntryCacheElement &AllocateElement(std::string_view child_name) __TA_REQUIRES(lock_);
  void DeallocateElement(const ElementRefPtr &element) __TA_REQUIRES(lock_);

  ElementRefPtr FindElementRefPtr(std::string_view child_name) const __TA_REQUIRES(lock_);
  DirEntryCacheElement *FindElement(std::string_view child_name) __TA_REQUIRES(lock_);

  std::map<EntryKey, ElementRefPtr> map_ __TA_GUARDED(lock_);
  mutable std::mutex lock_;
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_DIR_ENTRY_CACHE_H_
