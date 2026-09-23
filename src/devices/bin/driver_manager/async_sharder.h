// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_ASYNC_SHARDER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_ASYNC_SHARDER_H_

#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

namespace driver_manager {

// Helper class to make sending out concurrent async requests and making a callback when they have
// all finished easier.
class AsyncSharder {
 public:
  AsyncSharder(size_t count, fit::callback<void(zx::result<>)> complete_callback);
  ~AsyncSharder();
  void CompleteShard();
  void CompleteShardError(zx_status_t status);

 private:
  size_t remaining_;
  fit::callback<void(zx::result<>)> complete_callback_;
};

// Helper class to make sending out concurrent async requests and making a callback when they have
// all finished easier. This variant collects the results from the shards and gives it to the
// completion callback in a vector.
template <typename T>
class ResultAsyncSharder {
 public:
  ResultAsyncSharder(size_t count,
                     fit::callback<void(zx::result<std::vector<T>>)> complete_callback)
      : complete_callback_(std::move(complete_callback)), remaining_(count) {}

  ~ResultAsyncSharder() { ZX_ASSERT_MSG(remaining_ == 0, "Sharder not complete %zu", remaining_); }

  void CompleteShard(T result) {
    results_.push_back(std::forward<T>(result));
    remaining_--;
    if (remaining_ == 0 && complete_callback_) {
      complete_callback_(zx::ok(std::forward<std::vector<T>>(results_)));
    }
  }

  void CompleteShardError(zx_status_t status) {
    remaining_--;
    if (complete_callback_) {
      complete_callback_(zx::error(status));
    }
  }

 private:
  fit::callback<void(zx::result<std::vector<T>>)> complete_callback_;
  size_t remaining_;
  std::vector<T> results_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_ASYNC_SHARDER_H
