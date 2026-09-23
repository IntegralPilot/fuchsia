// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_PROVIDER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_PROVIDER_H_

#include <memory>
#include <vector>

#include "lib/fit/function.h"
#include "src/developer/debug/zxdb/client/async_task.h"

namespace zxdb {

class Err;
class Frame;
class Session;

// Interface for language-specific async task tree fetchers.
class AsyncTaskProvider {
 public:
  virtual ~AsyncTaskProvider() = default;

  // Returns true if this provider can handle the given frame (e.g. it's a Rust frame with an
  // executor).
  virtual bool CanHandle(Frame* frame) const = 0;

  // Asynchronously fetches the async task tree starting from the given frame.
  virtual void GetTasks(
      Frame* frame,
      fit::callback<void(const Err&, std::vector<std::unique_ptr<AsyncTask>>)> cb) = 0;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_ASYNC_TASK_PROVIDER_H_
