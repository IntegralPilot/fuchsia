// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/shutdown_message.h"

#include <lib/zx/time.h>

#include <string>

#include "src/lib/fxl/strings/string_printf.h"

namespace forensics::feedback_data::system_log_recorder {

std::string ShutdownMessage(const zx::time_boot now) {
  const int secs = static_cast<int>(now.get() / 1000000000ULL);
  const int msecs = static_cast<int>((now.get() / 1000000ULL) % 1000ULL);
  return fxl::StringPrintf(
      "!!! [%05d.%03d] SYSTEM SHUTDOWN SIGNAL RECEIVED FURTHER LOGS ARE NOT GUARANTEED !!!\n", secs,
      msecs);
}

}  // namespace forensics::feedback_data::system_log_recorder
