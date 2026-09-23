// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_LOG_BUFFER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_LOG_BUFFER_H_

#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include <deque>
#include <optional>
#include <string>

#include "src/developer/forensics/feedback_data/log_source.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/developer/forensics/utils/storage_size.h"

namespace forensics::feedback {

// Stores up to |capacity| bytes of system log messages, dropping the earliest messages when the
// stored messages occupy too much space.
class LogBuffer : public feedback_data::LogSink {
 public:
  LogBuffer(StorageSize capacity, RedactorBase* redactor);

  virtual ~LogBuffer() = default;

  // Adds |message| to the buffer and drops messages as required to keep the total size under
  // |capacity|. Always returns true.
  //
  // Messages are assumed to be received mostly in order.
  bool Add(LogSink::MessageOr message) override;

  // Records the log stream was interrupted and clears the contents.
  void NotifyInterruption() override;

  // It's safe continue to writing to a LogBuffer if the log source has been interrupted.
  bool SafeAfterInterruption() const override { return true; }

  // Returns std::nullopt if the buffer is empty.
  std::optional<zx::time_boot> FirstTimestamp() const;

  // Returns std::nullopt if the buffer is empty.
  std::optional<zx::time_boot> LastTimestamp() const;

  std::string ToString();

  // Executes |action| after a message with a time greater than or equal to |timestamp| is received
  // or NotifyInterruption is called.
  void ExecuteAfter(zx::time_boot timestamp, ::fit::closure action);

 private:
  struct Message {
    Message(const LogSink::MessageOr& message, zx::time_boot default_timestamp);

    zx::time_boot timestamp;
    std::string msg;
  };

  void Sort();
  void RunActions(zx::time_boot timestamp);
  void EnforceCapacity();

  // Resets variables keeping track of the last message
  void ResetLastMessage();

  RedactorBase* redactor_;
  std::deque<Message> messages_;

  std::string last_msg_{};
  int32_t last_severity_{};
  std::vector<std::string> last_tags{};
  size_t last_msg_repeated_{0u};

  bool is_sorted_{true};

  std::multimap<zx::time_boot, ::fit::closure, std::greater<>> actions_at_time_;

  size_t size_{0u};
  const size_t capacity_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_LOG_BUFFER_H_
