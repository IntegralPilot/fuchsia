// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/disk_backed_logs_metadata.h"

#include <lib/syslog/cpp/macros.h>

#include <ranges>

#include <rapidjson/document.h>
#include <rapidjson/stringbuffer.h>
#include <rapidjson/writer.h>

#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/fxl/strings/string_number_conversions.h"

namespace forensics::feedback_data::system_log_recorder {
namespace {

constexpr char kMessageCountKey[] = "message_count";
constexpr char kDeduplicatedMessageCountKey[] = "deduplicated_message_count";
constexpr char kFirstTimestampKey[] = "first_timestamp_nanos";
constexpr char kLastTimestampKey[] = "last_timestamp_nanos";
constexpr char kFileStatsKey[] = "file_stats";

}  // namespace

DiskBackedLogsMetadata::DiskBackedLogsMetadata(std::map<size_t, LogStats> file_stats,
                                               const size_t first_file_number)
    : file_stats_(std::move(file_stats)), first_file_number_(first_file_number) {}

bool DiskBackedLogsMetadata::IsAtCapacity() const {
  return !file_stats_.empty() && file_stats_.begin()->first > first_file_number_;
}

uint64_t DiskBackedLogsMetadata::MessageCount() const {
  uint64_t count = 0;
  for (const auto& [file_num, stats] : file_stats_) {
    count += stats.message_count;
  }
  return count;
}

uint64_t DiskBackedLogsMetadata::DeduplicatedMessageCount() const {
  uint64_t count = 0;
  for (const auto& [file_num, stats] : file_stats_) {
    count += stats.deduplicated_message_count;
  }
  return count;
}

std::optional<zx::time_boot> DiskBackedLogsMetadata::FirstTimestamp() const {
  for (const auto& [file_num, stats] : file_stats_) {
    if (stats.first_timestamp.has_value()) {
      return stats.first_timestamp;
    }
  }
  return std::nullopt;
}

std::optional<zx::time_boot> DiskBackedLogsMetadata::LastTimestamp() const {
  for (const auto& [file_num, stats] : std::views::reverse(file_stats_)) {
    if (stats.last_timestamp.has_value()) {
      return stats.last_timestamp;
    }
  }
  return std::nullopt;
}

std::optional<DiskBackedLogsMetadata> DiskBackedLogsMetadata::FromFile(
    const std::string& path, const size_t first_file_number) {
  if (!files::IsFile(path)) {
    // The file may not exist at the beginning of a boot.
    return std::nullopt;
  }

  std::string content;
  if (!files::ReadFileToString(path, &content)) {
    FX_LOGS(WARNING) << "Failed to read disk backed logs metadata from: " << path;
    return std::nullopt;
  }

  rapidjson::Document doc;
  if (doc.Parse(content.c_str()).HasParseError() || !doc.IsObject()) {
    FX_LOGS(WARNING) << "Failed to parse disk backed logs metadata JSON from: " << path;
    return std::nullopt;
  }

  std::map<size_t, LogStats> file_stats;

  if (doc.HasMember(kFileStatsKey) && doc[kFileStatsKey].IsObject()) {
    for (auto it = doc[kFileStatsKey].MemberBegin(); it != doc[kFileStatsKey].MemberEnd(); ++it) {
      if (!it->name.IsString() || !it->value.IsObject()) {
        continue;
      }

      size_t file_num;
      if (!fxl::StringToNumberWithError(it->name.GetString(), &file_num)) {
        FX_LOGS(WARNING) << "Failed to parse file number from: " << it->name.GetString();
        continue;
      }

      const auto& stat_obj = it->value;
      LogStats stats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                     /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);
      if (stat_obj.HasMember(kMessageCountKey) && stat_obj[kMessageCountKey].IsUint64()) {
        stats.message_count = stat_obj[kMessageCountKey].GetUint64();
      }
      if (stat_obj.HasMember(kDeduplicatedMessageCountKey) &&
          stat_obj[kDeduplicatedMessageCountKey].IsUint64()) {
        stats.deduplicated_message_count = stat_obj[kDeduplicatedMessageCountKey].GetUint64();
      }
      if (stat_obj.HasMember(kFirstTimestampKey) && stat_obj[kFirstTimestampKey].IsInt64()) {
        stats.first_timestamp = zx::time_boot(stat_obj[kFirstTimestampKey].GetInt64());
      }
      if (stat_obj.HasMember(kLastTimestampKey) && stat_obj[kLastTimestampKey].IsInt64()) {
        stats.last_timestamp = zx::time_boot(stat_obj[kLastTimestampKey].GetInt64());
      }

      file_stats.emplace(file_num, stats);
    }
  }

  return DiskBackedLogsMetadata(std::move(file_stats), first_file_number);
}

bool DiskBackedLogsMetadata::ToFile(const std::string& path) const {
  if (file_stats_.empty()) {
    return false;
  }

  rapidjson::Document doc;
  doc.SetObject();
  rapidjson::Value file_stats_obj(rapidjson::kObjectType);

  for (const auto& [file_num, stats] : file_stats_) {
    rapidjson::Value key(std::to_string(file_num).c_str(), doc.GetAllocator());
    rapidjson::Value value(rapidjson::kObjectType);

    value.AddMember(kMessageCountKey, stats.message_count, doc.GetAllocator());
    value.AddMember(kDeduplicatedMessageCountKey, stats.deduplicated_message_count,
                    doc.GetAllocator());

    if (stats.first_timestamp.has_value()) {
      value.AddMember(kFirstTimestampKey, stats.first_timestamp->get(), doc.GetAllocator());
    }

    if (stats.last_timestamp.has_value()) {
      value.AddMember(kLastTimestampKey, stats.last_timestamp->get(), doc.GetAllocator());
    }

    file_stats_obj.AddMember(key, value, doc.GetAllocator());
  }

  doc.AddMember(kFileStatsKey, file_stats_obj, doc.GetAllocator());

  rapidjson::StringBuffer buffer;
  rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
  doc.Accept(writer);

  return files::WriteFileInTwoPhases(path, buffer.GetString(), files::GetDirectoryName(path));
}

void DiskBackedLogsMetadata::ReconcileWithExistingFiles(const std::vector<size_t>& existing_files) {
  std::map<size_t, LogStats> reconciled_stats;
  for (const size_t file_num : existing_files) {
    if (auto it = file_stats_.find(file_num); it != file_stats_.end()) {
      reconciled_stats.insert({file_num, it->second});
    } else {
      reconciled_stats.insert(
          {file_num, LogStats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                              /*first_timestamp=*/std::nullopt,
                              /*last_timestamp=*/std::nullopt)});
    }
  }
  file_stats_ = std::move(reconciled_stats);
}

void DiskBackedLogsMetadata::MergeInto(size_t file, const LogStats& stats) {
  auto it = file_stats_.find(file);
  if (it == file_stats_.end()) {
    file_stats_.insert({file, stats});
    return;
  }

  LogStats& current = it->second;
  current.message_count += stats.message_count;
  current.deduplicated_message_count += stats.deduplicated_message_count;
  if (!current.first_timestamp.has_value()) {
    current.first_timestamp = stats.first_timestamp;
  }
  if (stats.last_timestamp.has_value()) {
    current.last_timestamp = stats.last_timestamp;
  }
}

void DiskBackedLogsMetadata::NewStats(size_t file) {
  file_stats_.insert_or_assign(
      file, LogStats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                     /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt));
}

void DiskBackedLogsMetadata::RemoveStats(size_t file) { file_stats_.erase(file); }

void DiskBackedLogsMetadata::Clear() { file_stats_.clear(); }

bool DiskBackedLogsMetadata::Empty() const { return file_stats_.empty(); }

size_t DiskBackedLogsMetadata::NumFiles() const { return file_stats_.size(); }

size_t DiskBackedLogsMetadata::OldestFileNumber() const {
  FX_CHECK(!file_stats_.empty());
  return file_stats_.begin()->first;
}

size_t DiskBackedLogsMetadata::LatestFileNumber() const {
  FX_CHECK(!file_stats_.empty());
  return file_stats_.rbegin()->first;
}

size_t DiskBackedLogsMetadata::NextFileNumber() const {
  return file_stats_.empty() ? first_file_number_ : file_stats_.rbegin()->first + 1;
}

void DiskBackedLogsMetadata::LogToCobalt(cobalt::Logger& cobalt,
                                         std::optional<zx::duration> last_boot_uptime) const {
  const std::optional<zx::time_boot> last_log_timestamp = LastTimestamp();

  if (last_log_timestamp.has_value() && last_boot_uptime.has_value()) {
    const int64_t delta_ms =
        (zx::time_boot(last_boot_uptime->get()) - *last_log_timestamp).to_msecs();

    cobalt.LogEvent(cobalt::Event(
        cobalt::EventType::kInteger, cobalt_registry::kPreviousBootLogLastTimestampDeltaMetricId,
        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, delta_ms));
  }

  if (!IsAtCapacity()) {
    return;
  }

  cobalt.LogEvent(cobalt::Event(
      cobalt::EventType::kInteger,
      cobalt_registry::kPreviousBootLogDeduplicatedMessageCountAtCapacityMetricId,
      {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, DeduplicatedMessageCount()));

  cobalt.LogEvent(cobalt::Event(
      cobalt::EventType::kInteger, cobalt_registry::kPreviousBootLogMessageCountAtCapacityMetricId,
      {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, MessageCount()));

  const std::optional<zx::time_boot> first_log_timestamp = FirstTimestamp();

  if (first_log_timestamp.has_value() && last_log_timestamp.has_value() &&
      *last_log_timestamp >= *first_log_timestamp) {
    const zx::duration duration = *last_log_timestamp - *first_log_timestamp;
    cobalt.LogEvent(cobalt::Event(
        cobalt::EventType::kInteger, cobalt_registry::kPreviousBootLogDurationAtCapacityMetricId,
        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, duration.to_mins()));
  }
}

}  // namespace forensics::feedback_data::system_log_recorder
