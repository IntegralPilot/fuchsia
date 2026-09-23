// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_INSPECT_CPP_BOUNDED_LIST_NODE_H_
#define LIB_INSPECT_CPP_BOUNDED_LIST_NODE_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <algorithm>
#include <deque>
#include <mutex>

namespace inspect {

// `BoundedListNode` is a list of Inspect nodes with a maximum capacity.
//
// Nodes are rolled out in FIFO order as new nodes beyond `capacity()` are created.
//
// Nodes are named by insertion order: "0", "1", "2", ...
// For example,
//
// ```
// auto list = BoundedListNode(inspector.GetRoot().CreateChild("list"), 3);
// list.CreateEntry([](Node& n) { n.RecordInt("first", 1); });
// list.CreateEntry([](Node& n) { n.RecordInt("second", 2); });
// list.CreateEntry([](Node& n) { n.RecordInt("third", 3); });
// ```
//
// Would create the hierarchy:
// {
//     root: {
//         list: {
//             "0": { "first": 1 },
//             "1": { "second": 2 },
//             "2": { "third": 3 },
//         },
//     },
// }
//
// On a subsequent call to `list.CreateEntry(...)`, the node `"0"` would get pushed out,
// and node `"3"` would be inserted, representing that most recent call.
//
// `BoundedListNode` is thread-safe.
class BoundedListNode {
 public:
  // Construct a `BoundedListNode` with a fixed max capacity.
  BoundedListNode(inspect::Node&& n, uint64_t max_capacity)
      : inner_(new Inner(std::move(n), max_capacity)) {}

  BoundedListNode& operator=(BoundedListNode&& other) = default;
  BoundedListNode(BoundedListNode&& n) noexcept = default;

  // Copying inspect::Node is not allowed
  BoundedListNode(const BoundedListNode&) = delete;
  BoundedListNode() = delete;
  ~BoundedListNode() = default;

  // Check the max capacity of `this`.
  uint64_t capacity() const;

  // Create a new entry at the end of `this`, popping from the front if necessary.
  //
  // Safety: the reference passed to the provided callback will be valid so long as
  // the referent is present in `this`.
  void CreateEntry(std::function<void(inspect::Node&)> cb);

 private:
  class Inner {
   public:
    // Construct a `BoundedListNode::Inner` with a fixed max capacity.
    Inner(inspect::Node&& n, uint64_t max_capacity);

    // `Inner` must stay put in a std::unique_ptr until destruction.
    Inner(BoundedListNode&& n) = delete;
    Inner(const BoundedListNode&) = delete;
    Inner() = delete;
    ~Inner() = default;

    void CreateEntry(std::function<void(inspect::Node&)> cb);
    uint64_t capacity() const;

   private:
    // Take the lock on mutex_ and dispatch this from `CreateEntry`
    void LockedCreateEntry(std::function<void(inspect::Node&)> cb) __TA_REQUIRES(mutex_);

    // The node that holds the buffered children created and managed by `this`.
    inspect::Node list_root_;

    // The maximum capacity of the list.
    const uint64_t cap_;

    // The current index.
    uint64_t index_ = 0;

    // The mutex guarding `items_`.
    std::mutex mutex_;

    // The node-managing container.
    std::deque<inspect::Node> items_ __TA_GUARDED(mutex_);
  };

  // The thread-safe inner list.
  std::unique_ptr<Inner> inner_;
};
}  // namespace inspect

#endif  // LIB_INSPECT_CPP_BOUNDED_LIST_NODE_H_
