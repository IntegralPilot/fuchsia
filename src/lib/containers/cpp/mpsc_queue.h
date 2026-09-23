// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_CONTAINERS_CPP_MPSC_QUEUE_H_
#define SRC_LIB_CONTAINERS_CPP_MPSC_QUEUE_H_

#include <atomic>
#include <optional>

namespace containers {

// A lock free queue for multiple producers and a single consumer.
template <typename T>
class MpscQueue {
 public:
  MpscQueue() : cache_(nullptr), head_(nullptr) {}

  ~MpscQueue() { Clear(); }

  // Disallow copy, assign, and move.
  MpscQueue(MpscQueue&&) = delete;
  MpscQueue(const MpscQueue&) = delete;
  MpscQueue& operator=(MpscQueue&&) = delete;
  MpscQueue& operator=(const MpscQueue&) = delete;

  // Pushes a new element onto the queue.
  //
  // In any given thread, elements pushed first will be dequeued first. When
  // pushers on different threads contend it is not gauranteed that the thread
  // to call first will end up in the queue first.
  template <typename U>
  void Push(U&& element) {
    Cell* loaded_head;
    Cell* new_head = new Cell{.element = std::forward<T>(element)};
    do {
      loaded_head = head_.load();
      new_head->next = loaded_head;
    } while (!head_.compare_exchange_strong(loaded_head, new_head));
  }

  // Pops an element from the queue.
  //
  // This should only be called from the consumer thread.
  std::optional<T> Pop() {
    if (!cache_) {
      cache_ = TakeHead();
    }

    if (!cache_) {
      return std::nullopt;
    }

    T elem = std::move(cache_->element);
    Cell* to_delete = cache_;
    cache_ = cache_->next;
    delete to_delete;
    return elem;
  }

  // Drops all elements from the queue.
  //
  // This should only be called from the consumer thread.
  void Clear() {
    while (Pop()) {
    }
  }

 private:
  struct Cell {
    T element;
    Cell* next;
  };

  Cell* TakeHead() {
    Cell* node;
    do {
      node = head_.load();
    } while (!head_.compare_exchange_strong(node, nullptr));

    if (!node) {
      return nullptr;
    }

    Cell* prev = nullptr;
    while (node) {
      Cell* tmp = node;
      node = node->next;
      tmp->next = prev;
      prev = tmp;
    }

    return prev;
  }

  Cell* cache_;
  std::atomic<Cell*> head_;
};

}  // namespace containers

#endif  // SRC_LIB_CONTAINERS_CPP_MPSC_QUEUE_H_
