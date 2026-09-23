// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_LOG_STATS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_LOG_STATS_H_

#include <lib/zx/time.h>

#include <cstdint>
#include <optional>

namespace forensics::feedback_data::system_log_recorder {

// Metadata about a set of log messages.
struct LogStats {
  uint64_t message_count;
  uint64_t deduplicated_message_count;
  std::optional<zx::time_boot> first_timestamp;
  std::optional<zx::time_boot> last_timestamp;

  LogStats(uint64_t message_count, uint64_t deduplicated_message_count,
           std::optional<zx::time_boot> first_timestamp,
           std::optional<zx::time_boot> last_timestamp)
      : message_count(message_count),
        deduplicated_message_count(deduplicated_message_count),
        first_timestamp(first_timestamp),
        last_timestamp(last_timestamp) {}
};

}  // namespace forensics::feedback_data::system_log_recorder

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_LOG_STATS_H_
