// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_BLOCK_GROUP_REGISTRY_H_
#define SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_BLOCK_GROUP_REGISTRY_H_

#include <pthread.h>
#include <zircon/compiler.h>

#include <array>
#include <mutex>
#include <optional>
#include <thread>

#include "src/storage/lib/block_protocol/block-fifo.h"

namespace block_client {

// Assigns a group ID which is unique to each calling thread.
//
// This class is thread-safe, although it should not be accessed by
// more than MAX_TXN_GROUP_COUNT threads simultaneously.
class BlockGroupRegistry {
 public:
  // Acquire an ID for the group of the calling thread.
  groupid_t GroupID();

 private:
  std::mutex lock_;
  std::array<std::optional<pthread_t>, MAX_TXN_GROUP_COUNT> threads_ __TA_GUARDED(lock_) = {};
};

}  // namespace block_client

#endif  // SRC_STORAGE_LIB_BLOCK_CLIENT_CPP_BLOCK_GROUP_REGISTRY_H_
