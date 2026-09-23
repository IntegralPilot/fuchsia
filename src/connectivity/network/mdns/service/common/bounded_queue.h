// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_COMMON_BOUNDED_QUEUE_H_
#define SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_COMMON_BOUNDED_QUEUE_H_

#include <sys/types.h>

#include <mutex>
#include <queue>

#include "src/lib/fxl/synchronization/thread_annotations.h"

// A generic Queue implementation which limits number of items
template <typename T>
class BoundedQueue {
 public:
  explicit BoundedQueue(size_t max_size) : max_size_(std::max(static_cast<size_t>(1), max_size)) {}

  // Add entry to BoundedQueue. If queue is already full
  // an item from front of the queue will be removed.
  template <typename... Args>
  T& AddEntry(Args&&... args) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (queue_.size() == max_size_) {
      queue_.pop();
    }
    queue_.emplace(std::forward<Args>(args)...);
    return queue_.back();
  }

 private:
  size_t max_size_;
  std::mutex mutex_;
  std::queue<T> queue_ FXL_GUARDED_BY(mutex_);
};

#endif  // SRC_CONNECTIVITY_NETWORK_MDNS_SERVICE_COMMON_BOUNDED_QUEUE_H_
