// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENT_PROVIDERS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENT_PROVIDERS_H_

#include <lib/sys/cpp/service_directory.h>

#include <memory>
#include <optional>
#include <set>
#include <string>

#include "src/developer/forensics/feedback/attachments/attachment_manager.h"
#include "src/developer/forensics/feedback/attachments/file_backed_provider.h"
#include "src/developer/forensics/feedback/attachments/inspect.h"
#include "src/developer/forensics/feedback/attachments/kernel_log.h"
#include "src/developer/forensics/feedback/attachments/previous_boot_inspect.h"
#include "src/developer/forensics/feedback/attachments/previous_boot_log.h"
#include "src/developer/forensics/feedback/attachments/process_tree.h"
#include "src/developer/forensics/feedback/attachments/system_log.h"
#include "src/developer/forensics/feedback_data/inspect_data_budget.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/backoff/backoff.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics::feedback {

// Wraps the annotations providers Feedback uses and the component's AttachmentManager.
class AttachmentProviders {
 public:
  AttachmentProviders(async_dispatcher_t* dispatcher,
                      std::shared_ptr<sys::ServiceDirectory> services,
                      std::shared_ptr<sys::ServiceDirectory> system_log_recorder_services,
                      std::optional<zx::duration> delete_previous_boot_log_at,
                      timekeeper::Clock* clock, RedactorBase* redactor,
                      feedback_data::InspectDataBudget* inspect_data_budget,
                      std::set<std::string> allowlist, cobalt::Logger* cobalt, zx::job root_job);

  AttachmentManager* GetAttachmentManager() { return &attachment_manager_; }

  static std::unique_ptr<backoff::Backoff> AttachmentProviderBackoff();

 private:
  LogBuffer log_buffer_;
  KernelLog kernel_log_;
  std::unique_ptr<AttachmentProvider> system_log_;
  Inspect inspect_;
  PreviousBootInspect previous_boot_inspect_;
  PreviousBootLog previous_boot_log_;
  FileBackedProvider previous_boot_kernel_log_;
  FileBackedProvider kernel_boot_options_;
  ProcessTree process_tree_;

  AttachmentManager attachment_manager_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENT_PROVIDERS_H_
