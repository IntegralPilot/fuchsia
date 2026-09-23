// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/inspect/cpp/bounded_list_node.h>

namespace inspect {
uint64_t BoundedListNode::capacity() const { return inner_->capacity(); }

void BoundedListNode::CreateEntry(std::function<void(inspect::Node&)> cb) {
  inner_->CreateEntry(std::move(cb));
}

BoundedListNode::Inner::Inner(inspect::Node&& n, uint64_t max_capacity)
    : list_root_(std::move(n)), cap_(max_capacity) {
  ZX_ASSERT(cap_ > 0);
}

void BoundedListNode::Inner::CreateEntry(std::function<void(inspect::Node&)> cb) {
  std::lock_guard lock{mutex_};
  LockedCreateEntry(std::move(cb));
}

void BoundedListNode::Inner::LockedCreateEntry(std::function<void(inspect::Node&)> cb) {
  while (items_.size() >= cap_) {
    items_.pop_front();
  }

  items_.push_back(list_root_.CreateChild(std::to_string(index_++)));
  items_.back().AtomicUpdate(std::move(cb));
}

uint64_t BoundedListNode::Inner::capacity() const { return cap_; }
}  // namespace inspect
