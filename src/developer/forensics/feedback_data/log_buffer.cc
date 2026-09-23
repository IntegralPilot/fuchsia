// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/log_buffer.h"

#include <lib/fit/defer.h>

#include <string>
#include <vector>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/log_source.h"
#include "src/developer/forensics/utils/log_format.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics::feedback {
namespace {

constexpr int32_t kDefaultLogSeverity = 0;
const std::vector<std::string> kDefaultTags = {};

size_t AppendRepeated(const size_t last_msg_repeated, std::string& append_to) {
  const std::string repeated_str =
      last_msg_repeated == 1
          ? feedback_data::kRepeatedOnceFormatStr
          : fxl::StringPrintf(feedback_data::kRepeatedFormatStr, last_msg_repeated);

  append_to.append(repeated_str);
  return repeated_str.size();
}

}  // namespace

LogBuffer::LogBuffer(const StorageSize capacity, RedactorBase* redactor)
    : redactor_(redactor), capacity_(capacity.ToBytes()) {}

std::optional<zx::time_boot> LogBuffer::FirstTimestamp() const {
  return messages_.empty() ? std::nullopt : std::make_optional(messages_.front().timestamp);
}

std::optional<zx::time_boot> LogBuffer::LastTimestamp() const {
  return messages_.empty() ? std::nullopt : std::make_optional(messages_.back().timestamp);
}

bool LogBuffer::Add(LogSink::MessageOr message) {
  if (message.is_ok()) {
    redactor_->Redact(message.value().msg);
    for (std::string& tag : message.value().tags) {
      redactor_->Redact(tag);
    }
  } else {
    redactor_->Redact(message.error());
  }

  // Assume timestamp 0 if no messages have been added yet.
  const zx::time_boot last_timestamp =
      (messages_.empty()) ? zx::time_boot(0) : messages_.back().timestamp;
  const std::string& msg = (message.is_ok()) ? message.value().msg : message.error();
  const int32_t& severity = (message.is_ok()) ? message.value().severity : kDefaultLogSeverity;
  const std::vector<std::string>& tags = (message.is_ok()) ? message.value().tags : kDefaultTags;

  // Adds a new message to |messages_| and updates internal accounting.
  auto AddNew = [this, &message, &msg, &severity, &tags, last_timestamp] {
    messages_.emplace_back(message, last_timestamp);
    size_ += messages_.back().msg.size();

    last_msg_ = msg;
    last_severity_ = severity;
    last_tags = tags;
    last_msg_repeated_ = 0;
    is_sorted_ &= messages_.back().timestamp >= last_timestamp;

    return true;
  };

  const zx::time_boot action_timestamp = (message.is_ok()) ? message.value().time : last_timestamp;
  auto on_return = ::fit::defer([this, action_timestamp] {
    RunActions(action_timestamp);
    EnforceCapacity();
  });

  if (messages_.empty()) {
    return AddNew();
  }

  // The most recent message is repeated, don't need to create new data.
  if (last_msg_ == msg && last_severity_ == severity && last_tags == tags) {
    ++last_msg_repeated_;
    return true;
  }

  // Inject a signal the most previously added message was repeated.
  if (last_msg_repeated_ > 0) {
    size_ += AppendRepeated(last_msg_repeated_, messages_.back().msg);
  }

  return AddNew();
}

void LogBuffer::NotifyInterruption() {
  messages_.clear();
  ResetLastMessage();
  is_sorted_ = true;
  size_ = 0u;

  // Executing and deleting all remaining actions is safe because non-SystemLog controlled
  // interruptions aren't expected to occur.
  for (auto& [_, action] : actions_at_time_) {
    action();
  }
  actions_at_time_.clear();
}

std::string LogBuffer::ToString() {
  // Ensure messages appear in time order.
  Sort();

  std::string out;
  out.reserve(size_);
  for (const Message& message : messages_) {
    out.append(message.msg);
  }

  // Inject a signal the last message was repeated because the signal doesn't exist in the
  // log yet.
  if (last_msg_repeated_ > 0) {
    AppendRepeated(last_msg_repeated_, out);
  }

  return out;
}

void LogBuffer::ExecuteAfter(const zx::time_boot uptime, ::fit::closure action) {
  actions_at_time_.insert({uptime, std::move(action)});
}

void LogBuffer::Sort() {
  // No sort is needed.
  if (is_sorted_) {
    return;
  }

  // Inject a signal the last message was repeated because the sort may change which message is
  // last.
  if (last_msg_repeated_ > 0) {
    size_ += AppendRepeated(last_msg_repeated_, messages_.back().msg);
  }

  std::stable_sort(messages_.begin(), messages_.end(), [](const Message& lhs, const Message& rhs) {
    return lhs.timestamp < rhs.timestamp;
  });
  is_sorted_ = true;

  // Reset the message last added.
  //
  // Note: info used to deduplicate messages is lost; it has not yet been proven important enough
  // in the system log to justify the cost of identifying what the original msg was and
  // aggregating all adjacent messages that match it. For example, it may be possible to see the
  // sequence:
  //
  // LOG MESSAGE A
  // !!! MESSAGE REPEATED 3 MORE TIMES!!!
  // LOG MESSAGE A
  //
  // in a final system log.
  ResetLastMessage();
}

void LogBuffer::RunActions(const zx::time_boot timestamp) {
  for (auto it = actions_at_time_.lower_bound(timestamp); it != actions_at_time_.end();) {
    it->second();
    actions_at_time_.erase(it++);
  }
}

void LogBuffer::EnforceCapacity() {
  if (size_ <= capacity_) {
    return;
  }

  // Ensure messages are dropped in time order.
  Sort();
  while (size_ > capacity_ && !messages_.empty()) {
    size_ -= messages_.front().msg.size();
    messages_.pop_front();
  }
}

void LogBuffer::ResetLastMessage() {
  last_msg_ = "";
  last_severity_ = 0;
  last_tags = {};
  last_msg_repeated_ = 0u;
}

LogBuffer::Message::Message(const LogSink::MessageOr& message, zx::time_boot default_timestamp)
    : timestamp(message.is_ok() ? message.value().time : default_timestamp),
      msg(message.is_ok() ? Format(message.value())
                          : fxl::StringPrintf("!!! Failed to format chunk: %s !!!\n",
                                              message.error().c_str())) {}

}  // namespace forensics::feedback
