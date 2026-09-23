// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_CLIENT_TEST_UTIL_INFLIGHT_LIST_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_CLIENT_TEST_UTIL_INFLIGHT_LIST_H_

#include <lib/magma/magma.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/macros.h>
#include <lib/magma/util/status.h>

#include <deque>

namespace magma {

// A convenience class for maintaining a list of inflight command buffers,
// by reading completed buffer ids from the notification channel.
// Caution, this approach only works for drivers that report completions
// in this format.
// Note, this class is not threadsafe.
class InflightList {
 public:
  InflightList() {}

  void add(uint64_t buffer_id) { buffers_.push_back(buffer_id); }

  void release(uint64_t buffer_id) {
    auto iter = std::find(buffers_.begin(), buffers_.end(), buffer_id);
    MAGMA_DASSERT(iter != buffers_.end());
    buffers_.erase(iter);
  }

  size_t size() { return buffers_.size(); }

  bool is_inflight(uint64_t buffer_id) {
    return std::find(buffers_.begin(), buffers_.end(), buffer_id) != buffers_.end();
  }

  // Wait for a completion; returns true if a completion was
  // received before |timeout_ms|.
  magma::Status WaitForCompletion(magma_connection_t connection, int64_t timeout_ns) {
    magma_poll_item_t item = {
        .handle = magma_connection_get_notification_channel_handle(connection),
        .type = MAGMA_POLL_TYPE_HANDLE,
        .condition = MAGMA_POLL_CONDITION_READABLE};
    return magma::Status(magma_poll(&item, 1, timeout_ns));
  }

  // Read all outstanding completions and update the inflight list.
  void ServiceCompletions(magma_connection_t connection) {
    uint64_t buffer_ids[8];
    uint64_t bytes_available = 0;
    magma_bool_t more_data = false;
    while (true) {
      magma_status_t status = magma_connection_read_notification_channel(
          connection, buffer_ids, sizeof(buffer_ids), &bytes_available, &more_data);
      if (status != MAGMA_STATUS_OK) {
        MAGMA_DLOG("magma_read_notification_channel returned %d", status);
        return;
      }
      if (bytes_available == 0)
        return;
      MAGMA_DASSERT(bytes_available % sizeof(uint64_t) == 0);
      for (uint32_t i = 0; i < bytes_available / sizeof(uint64_t); i++) {
        MAGMA_DASSERT(is_inflight(buffer_ids[i]));
        release(buffer_ids[i]);
      }
      if (!more_data)
        return;
    }
  }

 private:
  std::deque<uint64_t> buffers_;
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_CLIENT_TEST_UTIL_INFLIGHT_LIST_H_
