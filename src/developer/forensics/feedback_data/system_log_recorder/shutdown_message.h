// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_SHUTDOWN_MESSAGE_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_SHUTDOWN_MESSAGE_H_

#include <lib/zx/time.h>

#include <string>

namespace forensics::feedback_data::system_log_recorder {

// Returns the formatted string indicating the system is shutting down and doesn't guarantee any
// more logs.
std::string ShutdownMessage(zx::time_boot now);

}  // namespace forensics::feedback_data::system_log_recorder

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_SYSTEM_LOG_RECORDER_SHUTDOWN_MESSAGE_H_
