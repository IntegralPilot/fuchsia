// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/metrics.h"

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"

namespace forensics::feedback {
namespace {

const auto* const kTimedOutMetrics = new std::map<std::string, cobalt::TimedOutData>({
    {feedback_data::kAttachmentLogKernel, cobalt::TimedOutData::kKernelLog},
    {feedback_data::kAttachmentLogSystem, cobalt::TimedOutData::kSystemLog},
    {feedback_data::kAttachmentInspect, cobalt::TimedOutData::kInspect},
});

}  // namespace

AttachmentMetrics::AttachmentMetrics(cobalt::Logger* cobalt) : cobalt_(cobalt) {}

void AttachmentMetrics::LogMetrics(const Attachments& attachments) {
  std::set<cobalt::TimedOutData> to_log;
  for (const auto& [k, v] : attachments) {
    if (v.HasError() && v.Error() == Error::kTimeout && kTimedOutMetrics->contains(k)) {
      to_log.insert(kTimedOutMetrics->at(k));
    }
  }

  for (const cobalt::TimedOutData metric : to_log) {
    cobalt_->LogOccurrence(metric);
  }
}

}  // namespace forensics::feedback
