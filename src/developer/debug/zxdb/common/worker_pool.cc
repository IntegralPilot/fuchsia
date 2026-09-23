// Copyright 2018 The Chromium Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/common/worker_pool.h"

WorkerPool::WorkerPool(size_t thread_count) : should_stop_processing_(false) {
  threads_.reserve(thread_count);
  for (size_t i = 0; i < thread_count; ++i) {
    threads_.emplace_back([this]() { Worker(); });
  }
}

WorkerPool::~WorkerPool() {
  {
    std::unique_lock<std::mutex> queue_lock(queue_mutex_);
    should_stop_processing_ = true;
  }

  pool_notifier_.notify_all();

  for (auto& task_thread : threads_) {
    task_thread.join();
  }
}

void WorkerPool::PostTask(std::function<void()> work) {
  {
    std::unique_lock<std::mutex> queue_lock(queue_mutex_);
    task_queue_.emplace(std::move(work));
  }

  pool_notifier_.notify_one();
}

void WorkerPool::Worker() {
  for (;;) {
    std::function<void()> task;

    {
      std::unique_lock<std::mutex> queue_lock(queue_mutex_);

      pool_notifier_.wait(queue_lock,
                          [this]() { return (!task_queue_.empty()) || should_stop_processing_; });

      if (should_stop_processing_ && task_queue_.empty())
        return;

      task = std::move(task_queue_.front());
      task_queue_.pop();
    }

    task();
  }
}
