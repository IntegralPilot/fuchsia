// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_SYSTEM_TIME_TRACKER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_SYSTEM_TIME_TRACKER_H_

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/zx/time.h>

#include <optional>
#include <string>

#include "src/lib/timekeeper/clock.h"

namespace forensics::feedback {

struct SystemTime {
  std::optional<zx::duration> uptime;
  std::optional<zx::duration> runtime;
};

// Parses the previous system time from the given file containing JSON. Returns std::nullopt on
// error.
std::optional<SystemTime> GetPreviousSystemTime(const std::string& path);

// Periodically persists the current uptime and runtime to a file in milliseconds. Used as a
// fallback if the kernel's reboot log isn't available.
class SystemTimeTracker {
 public:
  SystemTimeTracker(async_dispatcher_t* dispatcher, timekeeper::Clock* clock,
                    zx::duration write_period, std::string write_path);

  // Starts periodically recording the system time.
  void Start();

  // Forces a write of the current system time.
  void RecordSystemShutdownSignal();

 private:
  void WriteTimeTask();
  void WriteUptimeAndRuntime();

  async_dispatcher_t* dispatcher_;
  timekeeper::Clock* clock_;
  zx::duration write_period_;
  std::string write_path_;

  async::TaskClosureMethod<SystemTimeTracker, &SystemTimeTracker::WriteTimeTask> write_time_task_{
      this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_SYSTEM_TIME_TRACKER_H_
