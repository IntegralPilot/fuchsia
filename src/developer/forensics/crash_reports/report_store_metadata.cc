// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/report_store_metadata.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <filesystem>
#include <optional>
#include <vector>

#include "src/developer/forensics/crash_reports/program_shortname.h"
#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/fxl/strings/string_number_conversions.h"

namespace forensics {
namespace crash_reports {

namespace fs = std::filesystem;

ReportStoreMetadata::ReportStoreMetadata(std::string report_store_root, const StorageSize max_size)
    : report_store_root_(std::move(report_store_root)),
      max_size_(max_size),
      current_size_(StorageSize::Bytes(0u)),
      is_directory_usable_(false) {
  RecreateFromAndCleanupFilesystem();
}

bool ReportStoreMetadata::RecreateFromAndCleanupFilesystem() {
  current_size_ = StorageSize::Bytes(0u);
  report_metadata_.clear();
  program_metadata_.clear();

  if (!files::IsDirectory(report_store_root_) && !files::CreateDirectory(report_store_root_)) {
    FX_LOGS(WARNING) << "Failed to create " << report_store_root_;
    is_directory_usable_ = false;
    return false;
  }

  std::vector<fs::path> invalid_paths;
  for (const fs::directory_entry& program_dir : fs::directory_iterator(report_store_root_)) {
    if (!files::IsDirectory(program_dir.path())) {
      FX_LOGS(WARNING) << "Unexpectedly not a program directory. Deleting: " << program_dir.path();
      invalid_paths.push_back(program_dir.path());
      continue;
    }

    const std::optional<ProgramShortname> program_shortname =
        ProgramShortname::Create(program_dir.path().filename());
    if (!program_shortname.has_value()) {
      FX_LOGS(WARNING) << "Unexpectedly not a valid program shortname. Deleting: "
                       << program_dir.path();
      invalid_paths.push_back(program_dir.path());
      continue;
    }

    for (const fs::directory_entry& report_dir : fs::directory_iterator(program_dir)) {
      if (!files::IsDirectory(report_dir.path())) {
        FX_LOGS(WARNING) << "Unexpectedly not a report directory. Deleting: " << report_dir.path();
        invalid_paths.push_back(report_dir.path());
        continue;
      }

      ReportId report_id;
      if (!fxl::StringToNumberWithError(report_dir.path().filename().string(), &report_id)) {
        FX_LOGS(WARNING) << "Report id unexpectedly not an integer: "
                         << report_dir.path().filename() << ". Deleting: " << report_dir.path();
        invalid_paths.push_back(report_dir.path());
        continue;
      }

      std::vector<std::string> attachments;
      StorageSize report_size = StorageSize::Bytes(0);
      for (const fs::directory_entry& attachment : fs::directory_iterator(report_dir)) {
        if (!files::IsFile(attachment.path())) {
          FX_LOGS(WARNING) << "Attachment file unexpectedly not a file. Deleting: "
                           << attachment.path();
          invalid_paths.push_back(attachment.path());
          continue;
        }

        attachments.push_back(attachment.path().filename());
        std::error_code ec;
        StorageSize attachment_size = StorageSize::Bytes(fs::file_size(attachment, ec));

        if (!ec) {
          report_size += attachment_size;
        }
      }

      current_size_ += report_size;

      report_metadata_[report_id].size = report_size;
      report_metadata_[report_id].dir = report_dir.path();
      report_metadata_[report_id].program_shortname = program_shortname->Value();
      report_metadata_[report_id].attachments = std::move(attachments);

      program_metadata_[program_shortname->Value()].dir = program_dir.path();
      program_metadata_[program_shortname->Value()].report_ids.push_back(report_id);
    }
  }

  for (const fs::path& path : invalid_paths) {
    if (!files::DeletePath(path, /*recursive=*/true)) {
      FX_LOGS(WARNING) << "Failed to delete: " << path;
    }
  }

  // Sort the reports such that the oldest report is at the front of the queue.
  for (auto& [_, metadata] : program_metadata_) {
    std::sort(metadata.report_ids.begin(), metadata.report_ids.end());
  }

  is_directory_usable_ = true;
  return true;
}

bool ReportStoreMetadata::IsDirectoryUsable() const { return is_directory_usable_; }

bool ReportStoreMetadata::Contains(const ReportId report_id) const {
  return report_metadata_.contains(report_id);
}

bool ReportStoreMetadata::Contains(const std::string& program) const {
  return program_metadata_.contains(program);
}

StorageSize ReportStoreMetadata::CurrentSize() const { return current_size_; }

StorageSize ReportStoreMetadata::RemainingSpace() const { return max_size_ - current_size_; }

const std::string& ReportStoreMetadata::RootDir() const { return report_store_root_; }

void ReportStoreMetadata::Add(const ReportId report_id, std::string program_shortname,
                              std::vector<std::string> attachments, const StorageSize size) {
  FX_CHECK(IsDirectoryUsable());
  current_size_ += size;

  program_metadata_[program_shortname].dir = fs::path(report_store_root_) / program_shortname;
  program_metadata_[program_shortname].report_ids.push_back(report_id);

  report_metadata_[report_id].size = size;
  report_metadata_[report_id].dir =
      fs::path(program_metadata_[program_shortname].dir) / std::to_string(report_id);
  report_metadata_[report_id].program_shortname = std::move(program_shortname);
  report_metadata_[report_id].attachments = std::move(attachments);
}

void ReportStoreMetadata::Delete(const ReportId report_id) {
  FX_CHECK(IsDirectoryUsable());
  FX_CHECK(Contains(report_id));

  const std::string program = ReportProgramShortname(report_id);
  std::deque<ReportId>& report_ids = program_metadata_[program].report_ids;
  report_ids.erase(std::find(report_ids.begin(), report_ids.end(), report_id));

  current_size_ -= report_metadata_[report_id].size;
  if (report_ids.empty()) {
    program_metadata_.erase(program);
  }
  report_metadata_.erase(report_id);
}

std::vector<std::string> ReportStoreMetadata::Programs() const {
  std::vector<std::string> programs;
  for (const auto& [program, _] : program_metadata_) {
    programs.push_back(program);
  }

  return programs;
}

std::vector<ReportId> ReportStoreMetadata::Reports() const {
  std::vector<ReportId> report_ids;
  for (const auto& [report_id, _] : report_metadata_) {
    report_ids.push_back(report_id);
  }

  return report_ids;
}

const std::deque<ReportId>& ReportStoreMetadata::ProgramReports(const std::string& program) const {
  FX_CHECK(program_metadata_.contains(program));
  return program_metadata_.at(program).report_ids;
}

const std::string& ReportStoreMetadata::ReportProgramShortname(const ReportId report_id) const {
  FX_CHECK(Contains(report_id));
  return report_metadata_.at(report_id).program_shortname;
}

const std::string& ReportStoreMetadata::ProgramDirectory(const std::string& program) const {
  FX_CHECK(program_metadata_.contains(program));
  return program_metadata_.at(program).dir;
}

const std::string& ReportStoreMetadata::ReportDirectory(const ReportId report_id) const {
  FX_CHECK(Contains(report_id));
  return report_metadata_.at(report_id).dir;
}

StorageSize ReportStoreMetadata::ReportSize(const ReportId report_id) const {
  FX_CHECK(Contains(report_id));
  return report_metadata_.at(report_id).size;
}

void ReportStoreMetadata::IncreaseSize(const ReportId report_id,
                                       const StorageSize additional_size) {
  FX_CHECK(Contains(report_id));

  current_size_ += additional_size;
  report_metadata_.at(report_id).size += additional_size;
}

std::vector<std::string> ReportStoreMetadata::ReportAttachments(ReportId report_id,
                                                                const bool absolute_paths) const {
  FX_CHECK(Contains(report_id));

  const ReportMetadata& report_metadata = report_metadata_.at(report_id);
  if (!absolute_paths) {
    return report_metadata.attachments;
  }

  std::vector<std::string> attachments;
  attachments.reserve(report_metadata.attachments.size());
  for (const std::string& attachment : report_metadata.attachments) {
    attachments.push_back(fs::path(report_metadata.dir) / attachment);
  }

  return attachments;
}

std::optional<std::string> ReportStoreMetadata::ReportAttachmentPath(
    ReportId report_id, const std::string& attachment_name) const {
  FX_CHECK(Contains(report_id));

  const ReportMetadata& report_metadata = report_metadata_.at(report_id);
  if (std::find(report_metadata.attachments.begin(), report_metadata.attachments.end(),
                attachment_name) == report_metadata.attachments.end()) {
    return std::nullopt;
  }

  return fs::path(report_metadata.dir) / attachment_name;
}

}  // namespace crash_reports
}  // namespace forensics
