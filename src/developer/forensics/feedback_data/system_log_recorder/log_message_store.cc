// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/log_message_store.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/log_format.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {
namespace {

const std::string kDroppedFormatStr = "!!! DROPPED %lu MESSAGES !!!\n";
constexpr int32_t kDefaultLogSeverity = 0;
const std::vector<std::string> kDefaultTags = {};

std::string MakeRepeatedWarning(const size_t message_count) {
  if (message_count == 2) {
    return kRepeatedOnceFormatStr;
  }

  return fxl::StringPrintf(kRepeatedFormatStr, message_count - 1);
}

std::string FormatError(const std::string& error) {
  return fxl::StringPrintf("!!! LOG PARSING ERROR: %s !!!", error.c_str());
}

}  // namespace

LogMessageStore::LogMessageStore(StorageSize max_block_capacity, StorageSize max_buffer_capacity,
                                 RedactorBase* redactor, std::unique_ptr<Encoder> encoder)
    : buffer_stats_(max_buffer_capacity),
      block_stats_(max_block_capacity),
      incremental_stats_(/*message_count=*/0, /*deduplicated_message_count=*/0,
                         /*first_timestamp=*/std::nullopt,
                         /*last_timestamp=*/std::nullopt),
      redactor_(redactor),
      encoder_(std::move(encoder)) {
  FX_CHECK(max_block_capacity >= max_buffer_capacity);
}

void LogMessageStore::AddToBuffer(const std::string& str) {
  const std::string encoded = encoder_->Encode(str);
  buffer_.push_back(encoded);
  block_stats_.Use(StorageSize::Bytes(encoded.size()));
  buffer_stats_.Use(StorageSize::Bytes(encoded.size()));
}

void LogMessageStore::ResetLastPushedMessage() {
  last_pushed_message_ = "";
  last_pushed_severity_ = 0;
  last_pushed_tags = {};
  last_pushed_message_count_ = 0;
}

bool LogMessageStore::Add(LogSink::MessageOr message) {
  TRACE_DURATION("feedback:io", "LogMessageStore::Add");

  if (message.is_ok()) {
    redactor_->Redact(message.value().msg);
    for (std::string& tag : message.value().tags) {
      redactor_->Redact(tag);
    }

    if (!incremental_stats_.first_timestamp.has_value()) {
      incremental_stats_.first_timestamp = message.value().time;
    }
    incremental_stats_.last_timestamp = message.value().time;
  }

  incremental_stats_.message_count++;

  const std::string& log_msg = message.is_ok() ? message.value().msg : message.error();
  const int32_t& log_severity = (message.is_ok()) ? message.value().severity : kDefaultLogSeverity;
  const std::vector<std::string>& log_tags =
      (message.is_ok()) ? message.value().tags : kDefaultTags;

  // 1. Early return if the incoming message, severity and tags are the same as last time.
  if (last_pushed_message_ == log_msg && last_pushed_severity_ == log_severity &&
      last_pushed_tags == log_tags) {
    last_pushed_message_count_++;
    return true;
  }

  incremental_stats_.deduplicated_message_count++;

  // 2. Push the repeated message if any.
  if (last_pushed_message_count_ > 1) {
    const std::string repeated_msg = MakeRepeatedWarning(last_pushed_message_count_);
    // We always add the repeated message to the buffer, even if it means going over bound as we
    // control its (small) size.
    AddToBuffer(repeated_msg);
  }
  // received new message, clear repeat variables.
  ResetLastPushedMessage();
  repeat_buffer_count_ = 0;

  // 3. Early return on full buffer when there is a rate limit.
  if (buffer_rate_limit_ && buffer_stats_.IsFull()) {
    ++num_messages_dropped_;
    return false;
  }

  // 4. Serialize incoming message.
  const std::string str =
      (message.is_ok()) ? Format(message.value()) : FormatError(message.error());

  // 5. Push the incoming message if below the limit or there is no rate limit; otherwise drop it.
  if (!buffer_rate_limit_ || buffer_stats_.CanUse(StorageSize::Bytes(str.size()))) {
    AddToBuffer(str);
    last_pushed_message_ = log_msg;
    last_pushed_severity_ = log_severity;
    last_pushed_tags = log_tags;
    last_pushed_message_count_ = 1;
    return true;
  }

  // We will drop the rest of the incoming messages until the next Consume(). This avoids trying
  // to squeeze in a shorter message that will wrongfully appear before the DROPPED message.
  buffer_stats_.MakeFull();
  ++num_messages_dropped_;
  return false;
}

LogMessageStore::ConsumeResult LogMessageStore::Consume() {
  TRACE_DURATION("feedback:io", "LogMessageStore::Consume");

  // Optionally log whether the last message was repeated. Stop logging if this warning message has
  // been logged consecutively for more than kRepeatedBuffers times.
  if (last_pushed_message_count_ > 1 && repeat_buffer_count_ < kMaxRepeatedBuffers) {
    AddToBuffer(MakeRepeatedWarning(last_pushed_message_count_));
    last_pushed_message_count_ = 1;
    repeat_buffer_count_++;
  }

  // Optionally log whether some messages were dropped.
  if (num_messages_dropped_ > 0) {
    AddToBuffer(fxl::StringPrintf(kDroppedFormatStr.c_str(), num_messages_dropped_));
    ResetLastPushedMessage();
  }

  // Optionally log important message at the end.
  if (to_append_.has_value()) {
    AddToBuffer(to_append_.value());
    to_append_ = std::nullopt;
    // We are changing the last message so we want to reset the last message pushed.
    ResetLastPushedMessage();
  }

  // We assume all messages end with a newline character.
  ConsumeResult result(fxl::JoinStrings(buffer_), /*end_of_block=*/false, incremental_stats_);

  buffer_.clear();
  buffer_stats_.Reset();
  num_messages_dropped_ = 0;
  incremental_stats_ = LogStats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                                /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);

  // Reset the encoder at the end of a block.
  if (block_stats_.IsFull()) {
    block_stats_.Reset();
    encoder_->Reset();
    // We reset the last message pushed and its count so that we don't have a block starting with a
    // repeated message without the actual message.
    ResetLastPushedMessage();
    result.end_of_block = true;
  } else {
    result.end_of_block = false;
  }

  return result;
}

void LogMessageStore::Reset() {
  buffer_.clear();
  buffer_stats_.Reset();
  block_stats_.Reset();
  encoder_->Reset();
  ResetLastPushedMessage();
  repeat_buffer_count_ = 0;
  num_messages_dropped_ = 0;
  incremental_stats_ = LogStats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                                /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);
  to_append_ = std::nullopt;
}

void LogMessageStore::AppendToEnd(const std::string& str) { to_append_ = str; }

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
