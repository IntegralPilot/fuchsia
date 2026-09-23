// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/snapshot_holder.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"

namespace view_tree {

SnapshotHolder::SnapshotHolder() : snapshot_(std::make_shared<const view_tree::Snapshot>()) {}

SnapshotHolder::Ref SnapshotHolder::GetSnapshot() {
  utils::CheckIsOnInputThread();
  FX_DCHECK(!ref_exists_) << "Attempting to check out snapshot while another reference exists!";
  std::shared_ptr<const Snapshot> snapshot;
  {
    std::scoped_lock lock(mutex_);
    snapshot = snapshot_;
  }
  TRACE_INSTANT("input", "SnapshotHolder::GetSnapshot", TRACE_SCOPE_THREAD, "sequence_number",
                snapshot->sequence_number);
  return Ref(*this, std::move(snapshot));
}

void SnapshotHolder::SetSnapshot(std::shared_ptr<const Snapshot> ptr) {
  utils::CheckIsOnMainThread();
  FX_DCHECK(ptr);
  std::scoped_lock lock(mutex_);
  FX_DCHECK(ptr->sequence_number > snapshot_->sequence_number)
      << "Snapshot sequence number must strictly monotonically increase! New: "
      << ptr->sequence_number << ", Old: " << snapshot_->sequence_number;
  snapshot_ = std::move(ptr);
}

SnapshotHolder::Ref::Ref(SnapshotHolder& holder, std::shared_ptr<const Snapshot> ptr)
    : holder_(&holder), ptr_(std::move(ptr)) {
  FX_DCHECK(ptr_) << "Invalid arguments to Ref constructor";
  holder_->ref_exists_ = true;
}

SnapshotHolder::Ref::Ref(Ref&& other) noexcept
    : holder_(other.holder_), ptr_(std::move(other.ptr_)) {
  other.holder_ = nullptr;
}

SnapshotHolder::Ref& SnapshotHolder::Ref::operator=(Ref&& other) noexcept {
  FX_DCHECK(!ptr_ == !holder_ && !other.ptr_ == !other.holder_)
      << "Pointer must be null if-and-only-if holder is null";
  if (this == &other) {
    return *this;
  }
  if (holder_) {
    holder_->ref_exists_ = false;
  }
  holder_ = other.holder_;
  ptr_ = std::move(other.ptr_);
  other.holder_ = nullptr;

  return *this;
}

SnapshotHolder::Ref::~Ref() {
  FX_DCHECK(!ptr_ == !holder_) << "Pointer must be null if-and-only-if holder is null";
  if (holder_) {
    utils::CheckIsOnInputThread();
    holder_->ref_exists_ = false;
  }
}

}  // namespace view_tree
