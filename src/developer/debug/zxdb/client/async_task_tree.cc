// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/async_task_tree.h"

#include "src/developer/debug/zxdb/client/stack.h"
#include "src/developer/debug/zxdb/client/thread.h"

namespace zxdb {

AsyncTaskTree::AsyncTaskTree(Delegate* delegate) : delegate_(delegate), weak_factory_(this) {}

AsyncTaskTree::~AsyncTaskTree() = default;

fxl::WeakPtr<AsyncTaskTree> AsyncTaskTree::GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

void AsyncTaskTree::Sync(Stack& stack,
                         fit::callback<void(const Err&, const Frame* frame)> callback) {
  stack.EnsureFrames({},
                     [weak_this = GetWeakPtr(), cb = std::move(callback)](const Err& err) mutable {
                       if (err.has_error()) {
                         cb(err, nullptr);
                         return;
                       } else if (!weak_this) {
                         cb(Err("AsyncTaskTree destroyed during task fetching"), nullptr);
                         return;
                       }

                       weak_this->delegate_->SyncAsyncTasks(std::move(cb));
                     });
}

void AsyncTaskTree::ForEach(fit::function<void(const TaskEntry&)> fn) const {
  std::vector<TaskEntry> stack;

  // Initialize the stack with the root tasks.
  for (const auto& root_task : GetRootTasks()) {
    stack.push_back({.task = root_task, .depth = 0});
  }

  while (!stack.empty()) {
    TaskEntry entry = stack.back();
    stack.pop_back();

    fn(entry);

    // Children are added at the end (they will be processed next).
    const auto& children = entry.task.GetChildren();
    for (const auto& child : children) {
      stack.push_back({.task = child, .depth = entry.depth + 1});
    }
  }
}

void AsyncTaskTree::SetTasks(std::vector<std::unique_ptr<AsyncTask>> tasks) {
  root_tasks_ = std::move(tasks);
}

std::vector<AsyncTask::Ref> AsyncTaskTree::GetRootTasks() const {
  std::vector<AsyncTask::Ref> ret;
  ret.reserve(root_tasks_.size());
  for (const auto& root_task : root_tasks_) {
    ret.push_back(*root_task);
  }
  return ret;
}

void AsyncTaskTree::ClearTasks() { root_tasks_.clear(); }

}  // namespace zxdb
