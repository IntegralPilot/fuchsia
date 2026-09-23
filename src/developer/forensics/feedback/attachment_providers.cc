// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachment_providers.h"

#include <utility>

#include "src/developer/forensics/feedback/attachments/disk_backed_system_log.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/lib/backoff/exponential_backoff.h"

namespace forensics::feedback {

AttachmentProviders::AttachmentProviders(
    async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
    std::shared_ptr<sys::ServiceDirectory> system_log_recorder_services,
    std::optional<zx::duration> delete_previous_boot_log_at, timekeeper::Clock* clock,
    RedactorBase* redactor, feedback_data::InspectDataBudget* inspect_data_budget,
    std::set<std::string> allowlist, cobalt::Logger* cobalt, zx::job root_job)
    : log_buffer_(feedback_data::kCurrentLogBufferSize, redactor),
      kernel_log_(dispatcher, services, AttachmentProviderBackoff(), redactor),
      system_log_([&]() -> std::unique_ptr<AttachmentProvider> {
        if (kEnableDiskBackedCurrentBootLogs) {
          return std::make_unique<DiskBackedSystemLog>(
              dispatcher, std::move(system_log_recorder_services),
              std::make_unique<backoff::ExponentialBackoff>(zx::sec(1), 2u, zx::min(5)), redactor,
              cobalt);
        }
        return std::make_unique<SystemLog>(dispatcher, services, clock, redactor,
                                           feedback_data::kActiveLoggingPeriod, cobalt,
                                           &log_buffer_);
      }()),
      inspect_(dispatcher, services, AttachmentProviderBackoff(), inspect_data_budget, redactor),
      previous_boot_inspect_(dispatcher, services, AttachmentProviderBackoff(), redactor,
                             kPreviousBootInspectPath),
      previous_boot_log_(dispatcher, clock, delete_previous_boot_log_at, kPreviousLogsFilePath),
      previous_boot_kernel_log_(forensics::feedback::kPreviousBootKernelLogPath,
                                /*warn_if_unavailable=*/false),
      kernel_boot_options_(kKernelBootOptionsPath),
      process_tree_(std::move(root_job)),
      attachment_manager_(
          dispatcher, clock, allowlist,
          {
              {feedback_data::kAttachmentLogKernel, &kernel_log_},
              {feedback_data::kAttachmentLogKernelPrevious, &previous_boot_kernel_log_},
              {feedback_data::kAttachmentLogSystem, system_log_.get()},
              {feedback_data::kAttachmentLogSystemPrevious, &previous_boot_log_},
              {feedback_data::kAttachmentInspect, &inspect_},
              {feedback_data::kAttachmentInspectPreviousBoot, &previous_boot_inspect_},
              {feedback_data::kAttachmentKernelBootOptions, &kernel_boot_options_},
              {feedback_data::kAttachmentProcessTree, &process_tree_},
          }) {
  if (allowlist.empty()) {
    FX_LOGS(WARNING)
        << "Attachment allowlist is empty, no platform attachments will be collected or returned";
  }
}

std::unique_ptr<backoff::Backoff> AttachmentProviders::AttachmentProviderBackoff() {
  return std::unique_ptr<backoff::Backoff>(
      new backoff::ExponentialBackoff(zx::min(1), 2u, zx::hour(1)));
}

}  // namespace forensics::feedback
