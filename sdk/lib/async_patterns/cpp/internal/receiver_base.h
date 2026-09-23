// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_INTERNAL_RECEIVER_BASE_H_
#define LIB_ASYNC_PATTERNS_CPP_INTERNAL_RECEIVER_BASE_H_

#include <lib/async/dispatcher.h>
#include <lib/async_patterns/cpp/internal/task_queue.h>
#include <lib/fit/nullable.h>

#include <memory>

namespace async_patterns::internal {

class ReceiverBase {
 public:
  ~ReceiverBase();

 protected:
  explicit ReceiverBase(async_dispatcher_t* dispatcher);

  async_dispatcher_t* dispatcher() const;
  TaskQueueHandle task_queue_handle() const { return TaskQueueHandle(task_queue_); }

 private:
  ReceiverBase(const ReceiverBase&) = delete;
  ReceiverBase& operator=(const ReceiverBase&) = delete;
  ReceiverBase(ReceiverBase&&) = delete;
  ReceiverBase& operator=(ReceiverBase&&) = delete;

  async_dispatcher_t* dispatcher_;
  std::shared_ptr<TaskQueue> task_queue_;
};

template <typename T>
constexpr inline bool is_stateless =
    std::is_trivially_copy_constructible_v<T> && std::is_trivially_destructible_v<T> &&
    std::is_class_v<T> && std::is_empty_v<T>;

}  // namespace async_patterns::internal

#endif  // LIB_ASYNC_PATTERNS_CPP_INTERNAL_RECEIVER_BASE_H_
