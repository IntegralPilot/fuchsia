// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/memfs/vnode.h"

#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/time.h>

#include <atomic>
#include <cstdint>
#include <ctime>

#include "src/storage/lib/vfs/cpp/paged_vnode.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/memfs/memfs.h"

namespace memfs {

std::atomic<uint64_t> Vnode::ino_ctr_ = 0;
std::atomic<uint64_t> Vnode::deleted_ino_ctr_ = 0;

Vnode::Vnode(Memfs& memfs)
    : fs::PagedVnode(memfs), ino_(ino_ctr_.fetch_add(1, std::memory_order_relaxed)) {
  std::timespec ts;
  if (std::timespec_get(&ts, TIME_UTC)) {
    create_time_ = modify_time_ = zx_time_from_timespec(ts);
  }
}

Vnode::~Vnode() { deleted_ino_ctr_.fetch_add(1, std::memory_order_relaxed); }

fs::VnodeAttributesQuery Vnode::SupportedMutableAttributes() const {
  return fs::VnodeAttributesQuery::kCreationTime | fs::VnodeAttributesQuery::kModificationTime |
         fs::VnodeAttributesQuery::kMode | fs::VnodeAttributesQuery::kUid |
         fs::VnodeAttributesQuery::kGid | fs::VnodeAttributesQuery::kRdev;
}

zx::result<> Vnode::UpdateAttributes(const fs::VnodeAttributesUpdate& attributes) {
  create_time_ = attributes.creation_time.value_or(create_time_);
  modify_time_ = attributes.modification_time.value_or(modify_time_);
  if (attributes.mode) {
    mode_ = attributes.mode;
  }
  if (attributes.uid) {
    uid_ = attributes.uid;
  }
  if (attributes.gid) {
    gid_ = attributes.gid;
  }
  if (attributes.rdev) {
    rdev_ = attributes.rdev;
  }
  return zx::ok();
}

void Vnode::Sync(SyncCallback closure) {
  // Since this filesystem is in-memory, all data is already up-to-date in
  // the underlying storage
  closure(ZX_OK);
}

void Vnode::UpdateModified() const {
  std::timespec ts;
  if (std::timespec_get(&ts, TIME_UTC)) {
    modify_time_ = zx_time_from_timespec(ts);
  } else {
    modify_time_ = 0;
  }
}

}  // namespace memfs
