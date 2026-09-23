// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_VIEW_TREE_SNAPSHOT_HOLDER_H_
#define SRC_UI_SCENIC_LIB_VIEW_TREE_SNAPSHOT_HOLDER_H_

#include <memory>
#include <mutex>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace view_tree {

// Holds a view tree snapshot and allows checking out a single reference at a time
// to ensure consistency within a call stack.
//
// Architectural note: this is an ideal extension point to allow input subsystems to access
// versioned view tree snapshots.  For example, an action which continues across multiple input
// events, such as a mouse-down-drag-up action, could process each event against a consistent
// snapshot, not whichever happens to be the latest.  No current use case requires this, so it
// seemed appropriate to write this note.
class SnapshotHolder {
 public:
  // A move-only reference to the snapshot. Only one can be checked out at a time.
  class Ref {
   public:
    Ref(const Ref&) = delete;
    Ref& operator=(const Ref&) = delete;
    Ref(Ref&& other) noexcept;
    Ref& operator=(Ref&& other) noexcept;
    ~Ref();

    const Snapshot* operator->() const noexcept { return ptr_.get(); }
    const Snapshot& operator*() const noexcept { return *ptr_; }

   private:
    friend class SnapshotHolder;
    Ref(SnapshotHolder& holder, std::shared_ptr<const Snapshot> ptr);

    SnapshotHolder* holder_ = nullptr;
    std::shared_ptr<const Snapshot> ptr_;
  };

  SnapshotHolder();
  ~SnapshotHolder() = default;

  // Gets a reference to the current snapshot.
  // Must be called on the input thread.
  Ref GetSnapshot();

  // Sets a new snapshot.
  // Must be called on the main thread.
  void SetSnapshot(std::shared_ptr<const Snapshot> ptr);

 private:
  friend class Ref;
  bool ref_exists_ = false;
  mutable std::mutex mutex_;
  std::shared_ptr<const Snapshot> snapshot_ FXL_GUARDED_BY(mutex_);
};

using SnapshotRef = SnapshotHolder::Ref;

}  // namespace view_tree

#endif  // SRC_UI_SCENIC_LIB_VIEW_TREE_SNAPSHOT_HOLDER_H_