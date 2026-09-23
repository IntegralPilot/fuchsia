// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_DISK_BACKED_LOGS_METADATA_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_DISK_BACKED_LOGS_METADATA_H_

#include <lib/zx/time.h>

#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>

#include "src/developer/forensics/feedback_data/system_log_recorder/log_stats.h"
#include "src/developer/forensics/utils/cobalt/logger.h"

namespace forensics::feedback_data::system_log_recorder {

// Provides aggregate data about the underlying per-file metadata. Also provides static functions
// for reading/writing the metadata to disk and for logging the metadata as Cobalt metrics.
class DiskBackedLogsMetadata {
 public:
  DiskBackedLogsMetadata(std::map<size_t, LogStats> file_stats, size_t first_file_number);

  static std::optional<DiskBackedLogsMetadata> FromFile(const std::string& path,
                                                        size_t first_file_number);

  // Serializes the metadata to JSON format and writes it to disk at `path`. Returns true on
  // success, or false if there are no stats to write or if writing fails.
  bool ToFile(const std::string& path) const;

  // Reconciles tracked stats with `existing_files`, retaining stats for existing files,
  // initializing default stats for newly discovered existing files, and removing stats for missing
  // files.
  void ReconcileWithExistingFiles(const std::vector<size_t>& existing_files);

  // Merges `stats` into the stats for `file`, accumulating message counts and updating timestamps
  // (preserving the initial `first_timestamp` and updating `last_timestamp`). If `file` is not
  // currently tracked, `stats` is inserted as a new entry.
  void MergeInto(size_t file, const LogStats& stats);

  // Initializes default (empty) stats for `file`, resetting any existing stats if `file` is already
  // tracked.
  void NewStats(size_t file);

  // Removes stats tracked for `file`.
  void RemoveStats(size_t file);

  // Clears all tracked file stats.
  void Clear();

  // Returns true if no per-file stats are currently tracked.
  bool Empty() const;

  // Returns the number of files currently tracked.
  size_t NumFiles() const;

  // Must not be called if Empty() is true.
  size_t OldestFileNumber() const;

  // Must not be called if Empty() is true.
  size_t LatestFileNumber() const;

  size_t NextFileNumber() const;

  bool IsAtCapacity() const;

  uint64_t MessageCount() const;

  uint64_t DeduplicatedMessageCount() const;

  std::optional<zx::time_boot> FirstTimestamp() const;

  std::optional<zx::time_boot> LastTimestamp() const;

  const std::map<size_t, LogStats>& FileStats() const { return file_stats_; }

  // Logs metadata metrics to Cobalt (e.g. log message counts and timestamp deltas).
  void LogToCobalt(cobalt::Logger& cobalt, std::optional<zx::duration> last_boot_uptime) const;

 private:
  // Using std::map (which maintains keys sorted in ascending order) is intentional:
  // methods such as OldestFileNumber(), LatestFileNumber(), FirstTimestamp(), and
  // LastTimestamp() rely on file_stats_ being ordered by file number.
  std::map<size_t, LogStats> file_stats_;
  size_t first_file_number_;
};

}  // namespace forensics::feedback_data::system_log_recorder

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_DISK_BACKED_LOGS_METADATA_H_
