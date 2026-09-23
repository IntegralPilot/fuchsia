// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_REPORT_H_
#define SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_REPORT_H_

#include <fuchsia/mem/cpp/fidl.h>

#include <map>
#include <memory>
#include <optional>
#include <string>

#include "src/developer/forensics/crash_reports/annotation_map.h"
#include "src/developer/forensics/crash_reports/program_shortname.h"
#include "src/developer/forensics/crash_reports/report_id.h"
#include "src/developer/forensics/utils/sized_data.h"

namespace forensics {
namespace crash_reports {

// An in-memory representation of a report that will be uploaded to the crash server.
class Report {
 public:
  // Return fpromise::ok with a Report unless there are issues reading a fuchsia::mem::Buffer.
  static fpromise::result<Report> MakeReport(
      ReportId report_id, const ProgramShortname& program_shortname,
      const AnnotationMap& annotations, std::map<std::string, fuchsia::mem::Buffer> attachments,
      std::string snapshot_uuid, std::optional<fuchsia::mem::Buffer> minidump,
      bool is_hourly_report = false);

  Report(ReportId report_id, const std::string& program_shortname, const AnnotationMap& annotations,
         std::map<std::string, SizedData> attachments, std::string snapshot_uuid,
         std::optional<SizedData> minidump, bool is_hourly_report = false);

  ReportId Id() const { return id_; }

  std::string ProgramShortname() const { return program_shortname_; }

  const AnnotationMap& Annotations() const { return annotations_; }
  AnnotationMap& Annotations() { return annotations_; }

  const std::map<std::string, SizedData>& Attachments() const { return attachments_; }
  std::map<std::string, SizedData>& Attachments() { return attachments_; }

  const std::optional<SizedData>& Minidump() const { return minidump_; }
  std::optional<SizedData>& Minidump() { return minidump_; }

  const std::string& SnapshotUuid() const { return snapshot_uuid_; }
  std::string& SnapshotUuid() { return snapshot_uuid_; }

  bool IsHourlyReport() const { return is_hourly_report_; }

  // Whether the key is reserved for use by Feedback and should not be used by clients. This
  // includes:
  //
  // - Files that will be stored by the ReportStore
  // - Files that will be attached to crash reports when uploaded
  //
  // Note that file names zipped into a snapshot are not necessarily reserved because they don't
  // always meet the conditions above.
  static bool IsReservedAttachmentKey(const std::string& key);

  // Whether the key is valid, i.e. isn't a reserved key and doesn't contain disallowed characters.
  static bool IsValidAttachmentKey(const std::string& key);

 private:
  ReportId id_;
  std::string program_shortname_;
  AnnotationMap annotations_;
  std::map<std::string, SizedData> attachments_;
  std::string snapshot_uuid_;
  std::optional<SizedData> minidump_;
  bool is_hourly_report_;
};

}  // namespace crash_reports
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_REPORT_H_
