// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_REPORT_UTIL_H_
#define SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_REPORT_UTIL_H_

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/fpromise/result.h>

#include <optional>
#include <string>

#include "src/developer/forensics/crash_reports/annotation_map.h"
#include "src/developer/forensics/crash_reports/product.h"
#include "src/developer/forensics/crash_reports/program_shortname.h"
#include "src/developer/forensics/crash_reports/report.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics {
namespace crash_reports {

// Methods to build annotations from various data collected during report creation.
AnnotationMap GetReportAnnotations(const feedback::Annotations& snapshot_annotations);
AnnotationMap GetReportAnnotations(Product product, const AnnotationMap& annotations);

// Builds the final report to add to the queue.
//
// * Most annotations are shared across all crash reports, e.g. the device uptime.
// * Some annotations are report-specific, e.g., Dart exception type.
// * Adds any annotations from |report|.
//
// * Some attachments are report-specific, e.g., Dart exception stack trace.
// * Adds any attachments from |report|.
fpromise::result<Report> MakeReport(fuchsia::feedback::CrashReport input_report,
                                    const ProgramShortname& program_shortname, ReportId report_id,
                                    const std::string& snapshot_uuid,
                                    const feedback::Annotations& snapshot_annotations,
                                    const std::optional<timekeeper::time_utc>& current_time,
                                    Product product, bool is_hourly_report);

}  // namespace crash_reports
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_REPORT_UTIL_H_
