// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/dir_entry_cache.h"

namespace f2fs {

DirEntryCache::DirEntryCache() {}

DirEntryCache::~DirEntryCache() { Reset(); }

void DirEntryCache::Reset() {
  std::lock_guard lock(lock_);
  map_.clear();
}

DirEntryCacheElement &DirEntryCache::AllocateElement(std::string_view child_name) {
  ElementRefPtr element = fbl::MakeRefCounted<DirEntryCacheElement>(child_name);
  ZX_ASSERT(element);
  map_[std::string(child_name)] = element;
  return *element;
}

ElementRefPtr DirEntryCache::FindElementRefPtr(std::string_view child_name) const {
  auto search = map_.find(std::string(child_name));
  if (search == map_.end()) {
    return nullptr;
  }

  return search->second;
}

DirEntryCacheElement *DirEntryCache::FindElement(std::string_view child_name) {
  ElementRefPtr element = FindElementRefPtr(child_name);
  if (element == nullptr) {
    return nullptr;
  }
  return element.get();
}

zx::result<DentryInfo> DirEntryCache::LookupDirEntry(std::string_view child_name) {
  if (IsDotOrDotDot(child_name)) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  std::lock_guard lock(lock_);
  DirEntryCacheElement *element = FindElement(child_name);
  if (element == nullptr) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  // The |element| may be evicted while the caller is using it.
  return zx::ok(element->GetInfo());
}

void DirEntryCache::UpdateDirEntry(std::string_view child_name, DentryInfo info) {
  if (IsDotOrDotDot(child_name)) {
    return;
  }

  std::lock_guard lock(lock_);
  DirEntryCacheElement *element = FindElement(child_name);
  if (element == nullptr) {
    return AllocateElement(child_name).SetInfo(info);
  }
  element->SetInfo(info);
}

void DirEntryCache::RemoveDirEntry(std::string_view child_name) {
  if (IsDotOrDotDot(child_name)) {
    return;
  }

  std::lock_guard lock(lock_);

  ElementRefPtr element = FindElementRefPtr(child_name);
  if (element != nullptr) {
    map_.erase(std::string(element->GetName()));
  }
}

bool DirEntryCache::IsElementInCache(std::string_view child_name) const {
  std::lock_guard lock(lock_);
  auto search = map_.find(std::string(child_name));
  return search != map_.end();
}

const std::map<EntryKey, ElementRefPtr> &DirEntryCache::GetMap() const {
  std::lock_guard lock(lock_);
  return map_;
}

}  // namespace f2fs
