// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/filing_result.h"

namespace forensics::crash_reports {

using fuchsia::feedback::FilingError;
using fuchsia::feedback::FilingSuccess;

fpromise::result<FilingSuccess, FilingError> ToFidlFilingResult(const FilingResult result) {
  switch (result) {
    case FilingResult::kReportUploaded:
      return fpromise::ok(FilingSuccess::REPORT_UPLOADED);
    case FilingResult::kReportOnDisk:
      return fpromise::ok(FilingSuccess::REPORT_ON_DISK);
    case FilingResult::kReportInMemory:
      return fpromise::ok(FilingSuccess::REPORT_IN_MEMORY);
    case FilingResult::kReportNotFiledUserOptedOut:
      return fpromise::ok(FilingSuccess::REPORT_NOT_FILED_USER_OPTED_OUT);
    case FilingResult::kInvalidArgsError:
      return fpromise::error(FilingError::INVALID_ARGS_ERROR);
    case FilingResult::kServerError:
      return fpromise::error(FilingError::SERVER_ERROR);
    case FilingResult::kPersistenceError:
      return fpromise::error(FilingError::PERSISTENCE_ERROR);
    case FilingResult::kQuotaReachedError:
      return fpromise::error(FilingError::QUOTA_REACHED_ERROR);
  }
}

}  // namespace forensics::crash_reports
