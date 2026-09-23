// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_REBOOT_LOG_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_REBOOT_LOG_H_

#include <lib/zx/time.h>

#include <optional>
#include <string>

#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"
#include "src/developer/forensics/utils/redact/redactor.h"

namespace forensics {
namespace feedback {

// Wrapper around a device's reboot log.
class RebootLog {
 public:
  static RebootLog ParseRebootLog(const std::string& zircon_reboot_log_path,
                                  const std::string& graceful_shutdown_info_path,
                                  const std::string& legacy_graceful_reboot_log_path,
                                  const std::string& previous_system_time_path,
                                  const std::string& previous_boot_kernel_log_path,
                                  const std::string& final_shutdown_info_path, bool not_a_fdr,
                                  bool supports_user_initiated_poweroffs,
                                  bool first_component_instance, RedactorBase* redactor);

  const std::string& RebootLogStr() const { return reboot_log_str_; }
  const FinalShutdownInfo& GetFinalShutdownInfo() const { return final_shutdown_info_; }

  // Exposed for testing purposes.
  RebootLog(FinalShutdownInfo final_shutdown_info, std::string reboot_log_str);

 private:
  FinalShutdownInfo final_shutdown_info_;
  std::string reboot_log_str_;
};

}  // namespace feedback
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_REBOOT_LOG_REBOOT_LOG_H_
