// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_STARTUP_ANNOTATIONS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_STARTUP_ANNOTATIONS_H_

#include <string>

#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"

namespace forensics::feedback {

// Gets annotations that are available immediately and synchronously when the component starts and
// never change while it is running.
Annotations GetStartupAnnotations(const FinalShutdownInfo& final_shutdown_info,
                                  SpontaneousRebootReason spontaneous_reboot_reason,
                                  const std::string& compilation_mode_path);

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_STARTUP_ANNOTATIONS_H_
