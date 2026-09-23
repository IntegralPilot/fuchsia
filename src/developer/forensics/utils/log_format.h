// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_LOG_FORMAT_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_LOG_FORMAT_H_

#include <fuchsia/logger/cpp/fidl.h>

#include <string>

namespace forensics {

// Format a log message as a string. Inserts a "dropped log" message if |message.dropped_logs| > 0.
std::string Format(const fuchsia::logger::LogMessage& message);

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_LOG_FORMAT_H_
