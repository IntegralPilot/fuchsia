// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/reader.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/time.h>

#include <algorithm>
#include <cctype>
#include <charconv>
#include <cmath>
#include <functional>
#include <string>
#include <vector>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {
namespace {

// Returns a view starting at the next line in |str| or an empty view if none are left.
std::string_view ToNextLine(std::string_view str) {
  const size_t pos = str.find_first_of('\n');
  return (pos != std::string_view::npos && pos != str.size()) ? str.substr(pos + 1)
                                                              : std::string_view();
}

// Returns true if |str| starts like a repeated message line.
bool MatchesRepeat(std::string_view str) {
  constexpr std::string_view kPrefix(kRepeatedStrPrefix);
  if (str.size() < kPrefix.size() || str.substr(0, kPrefix.size()) != kPrefix) {
    return false;
  }

  str.remove_prefix(kPrefix.size());
  return !str.empty() && std::isdigit(str.front());
}

// Returns true if |str| starts like a log message.
bool MatchesLogMessage(std::string_view str) {
  auto MatchOpenBr = [&str] {
    if (str.empty() || str.front() != '[') {
      return false;
    }

    str.remove_prefix(1);
    return true;
  };

  auto MatchDigitsPlus = [&str](const char c) {
    std::string_view new_str(str);
    while (!new_str.empty() && std::isdigit(new_str.front())) {
      new_str.remove_prefix(1);
    }

    if (new_str == str || (!new_str.empty() && new_str.front() != c)) {
      return false;
    }

    str = new_str.substr(1);
    return true;
  };

  return
      // The timestamp is like [$secs.$msecs]
      MatchOpenBr() && MatchDigitsPlus('.') && MatchDigitsPlus(']') &&
      // The process koid is like [$pid]
      MatchOpenBr() && MatchDigitsPlus(']') &&
      // The thread koid is like [$tid]
      MatchOpenBr() && MatchDigitsPlus(']');
}

// Splits message into 3 parts.
//  1) The portion of the message before the first sequence of repeated warnings.
//  2) The aggregated count of the sequence of repeated warnings.
//  3) The portion of the message after the first sequence of repeated warnings.
//
// Note, 2) and 3) will be default initialized if no repeated warnings are present.
std::tuple<std::string_view, size_t, std::string_view> SplitRepeatedCount(
    const std::string_view message) {
  constexpr std::string_view kPrefix(kRepeatedStrPrefix);

  std::string_view head;
  size_t repeated{0};
  std::string_view tail(message);

  // Find the first line matching the repeated message warning.
  while (!tail.empty()) {
    if (MatchesRepeat(tail)) {
      head = std::string_view(message.data(), tail.data() - message.data());
      break;
    }

    tail = ToNextLine(tail);
  }

  // No repeated warning found.
  if (tail.empty()) {
    return {message, 0, std::string_view()};
  }

  // Aggregate the repeated counts and find the end of the consecutive repeated message warnings.
  while (!tail.empty()) {
    if (!MatchesRepeat(tail)) {
      break;
    }

    std::string_view new_tail = tail;
    new_tail.remove_prefix(kPrefix.size());
    if (new_tail.empty() || !std::isdigit(new_tail.front())) {
      break;
    }

    int r{0};
    if (std::from_chars(new_tail.data(), new_tail.data() + new_tail.size(), r).ec != std::errc{}) {
      FX_LOGS(ERROR) << "Failed to extract repeat count from '" << new_tail << "'";
      break;
    }

    repeated += r;
    tail = ToNextLine(new_tail);
  }

  return {head, repeated, tail};
}

// Returns the messages which make up |log|.
std::vector<std::string_view> ParseMessages(const std::string_view log) {
  if (log.empty()) {
    return {};
  }

  // Overallocate to prevent residual memory when growing |messages|.
  //
  // 20,000 was selected because log files can be large and this is a large enough allocation to
  // allow the allocator to decommit the memory backing |messages| when the object is destroyed.
  // This is technically dedends implementation details of scudo, but is considered acceptable
  // due to the prior residual impact of processing the previous boot log
  // (https://fxbug.dev/42071246).
  std::vector<std::string_view> messages;
  messages.reserve(20000);

  // Consider the first line the start of a new message, regardless of whether or not it matches a
  // message.
  messages.emplace_back(log.substr(0, 1));

  std::string_view remainder = ToNextLine(log.substr(1));
  while (!remainder.empty()) {
    // Find either the start of the next message in |log| or the end of |log|.
    while (!remainder.empty() && !MatchesLogMessage(remainder)) {
      remainder = ToNextLine(remainder);
    }

    // "Grow" the last message's data.
    const char* const end = (!remainder.empty()) ? remainder.data() : log.data() + log.size();
    messages.back() = std::string_view(messages.back().data(), end - messages.back().data());

    if (remainder.empty()) {
      break;
    }

    messages.emplace_back(remainder.substr(0, 1));
    remainder.remove_prefix(1);
  }

  return messages;
}

std::string PostProcess(const std::string_view log) {
  std::vector<std::string_view> messages = ParseMessages(log);
  if (messages.empty()) {
    return "";
  }

  // Don't sort with the first message if it doesn't have a timestamp.
  auto begin = messages.begin();
  if (!MatchesLogMessage(*begin)) {
    ++begin;
  }

  std::stable_sort(
      begin, messages.end(), [](const std::string_view lhs, const std::string_view rhs) {
        const std::string_view lhs_timestamp = lhs.substr(lhs.find('['), lhs.find(']'));
        const std::string_view rhs_timestamp = rhs.substr(rhs.find('['), rhs.find(']'));

        // The timestamp format is "%05d.%03d" so longer strings mean larger timestamps and then
        // we only need to compare the strings if the length is the same.
        if (lhs_timestamp.size() != rhs_timestamp.size()) {
          return lhs_timestamp.size() < rhs_timestamp.size();
        }

        return lhs_timestamp < rhs_timestamp;
      });

  std::string final_log;
  final_log.reserve(log.size());

  // Appends |message| to |final_log| while recursively accumulating the repeated count for each
  // message.
  std::function<void(std::string_view)> AddMessage = [&final_log,
                                                      &AddMessage](const std::string_view message) {
    if (message.empty()) {
      return;
    }

    const auto [head, repeat_count, tail] = SplitRepeatedCount(message);

    final_log.append(head);

    if (repeat_count == 1) {
      final_log.append(kRepeatedOnceFormatStr);
    } else if (repeat_count > 1) {
      final_log.append(kRepeatedStrPrefix);
      final_log.append(std::to_string(repeat_count));
      final_log.append(kRepeatedStrSuffix);
    }

    AddMessage(tail);
  };

  for (const std::string_view message : messages) {
    AddMessage(message);
  }

  return final_log;
}

}  // namespace

fit::result<ReaderError, std::string> Concatenate(const std::string& logs_dir,
                                                  const StorageSize max_decompressed_size,
                                                  Decoder* decoder, float* compression_ratio) {
  // Set the default compression to NAN in case Concatenate() fails.
  *compression_ratio = NAN;

  if (!files::IsDirectory(logs_dir)) {
    FX_LOGS(WARNING) << "No logs directory found: " << logs_dir;
    return fit::error(ReaderError::kIoError);
  }

  std::vector<std::string> file_names;
  files::ReadDirContents(logs_dir, &file_names);

  // Remove the current directory from the files.
  file_names.erase(std::remove(file_names.begin(), file_names.end(), "."), file_names.end());

  // Sort the files based on the number the previous writer assigned them. The lower the number,
  // the older the file.
  std::sort(file_names.begin(), file_names.end(),
            [](const std::string& lhs, const std::string& rhs) {
              return std::strtoull(lhs.c_str(), nullptr, /*base=*/10) <
                     std::strtoull(rhs.c_str(), nullptr, /*base=*/10);
            });

  std::string uncompressed_log;
  uncompressed_log.reserve(max_decompressed_size.ToBytes());

  std::string buffer;
  buffer.reserve(max_decompressed_size.ToBytes());

  uint64_t total_compressed_log_size{0};
  for (const std::string& fname : file_names) {
    const std::string path = files::JoinPath(logs_dir, fname);
    if (!files::ReadFileToString(path, &buffer)) {
      continue;
    }

    total_compressed_log_size += buffer.size();
    uncompressed_log.append(decoder->Decode(buffer));
  }

  if (total_compressed_log_size == 0) {
    FX_LOGS(WARNING) << "The encoded log is empty";
    return fit::error(ReaderError::kIoError);
  }

  if (uncompressed_log.empty()) {
    FX_LOGS(WARNING) << "The decoded log is empty";
    return fit::error(ReaderError::kDecompressionError);
  }

  // Sort logs and combine messages for repeated logs.
  uncompressed_log = PostProcess(uncompressed_log);
  if (uncompressed_log.empty()) {
    FX_LOGS(WARNING) << "The post-processed log is empty";
    return fit::error(ReaderError::kDecompressionError);
  }

  // Compression ratio rounded up to the next decimal, e.g., 2.54x compression -> 2.6x.
  uint32_t decimal_ratio =
      (static_cast<uint32_t>(uncompressed_log.size()) * 10 - 1) / total_compressed_log_size + 1;
  *compression_ratio = (static_cast<float>(decimal_ratio)) / 10.0f;

  return fit::ok(uncompressed_log);
}

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
