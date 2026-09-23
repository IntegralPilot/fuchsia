// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/system_time_tracker.h"

#include <lib/syslog/cpp/macros.h>

#include <rapidjson/document.h>
#include <rapidjson/stringbuffer.h>
#include <rapidjson/writer.h>

#include "src/developer/forensics/feedback/constants.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"

namespace forensics::feedback {
namespace {

constexpr char kSystemTimeTrackerRuntimeKey[] = "runtime_ms";
constexpr char kSystemTimeTrackerUptimeKey[] = "uptime_ms";

}  // namespace

std::optional<SystemTime> GetPreviousSystemTime(const std::string& path) {
  std::string time_str;

  if (!files::ReadFileToString(path, &time_str)) {
    FX_LOGS(WARNING) << "Failed to read system time from: " << path;
    return std::nullopt;
  }

  rapidjson::Document doc;
  if (doc.Parse(time_str.c_str()).HasParseError() || !doc.IsObject()) {
    FX_LOGS(WARNING) << "Failed to parse system time JSON from: " << path;
    return std::nullopt;
  }

  SystemTime time;
  if (doc.HasMember(kSystemTimeTrackerUptimeKey) && doc[kSystemTimeTrackerUptimeKey].IsInt64()) {
    time.uptime = zx::msec(doc[kSystemTimeTrackerUptimeKey].GetInt64());
  }

  if (doc.HasMember(kSystemTimeTrackerRuntimeKey) && doc[kSystemTimeTrackerRuntimeKey].IsInt64()) {
    time.runtime = zx::msec(doc[kSystemTimeTrackerRuntimeKey].GetInt64());
  }

  return time;
}

SystemTimeTracker::SystemTimeTracker(async_dispatcher_t* dispatcher, timekeeper::Clock* clock,
                                     zx::duration write_period, std::string write_path)
    : dispatcher_(dispatcher),
      clock_(clock),
      write_period_(write_period),
      write_path_(std::move(write_path)) {}

void SystemTimeTracker::Start() {
  async::PostTask(dispatcher_, [this] { WriteTimeTask(); });
}

void SystemTimeTracker::RecordSystemShutdownSignal() {
  async::PostTask(dispatcher_, [this] { WriteUptimeAndRuntime(); });
}

void SystemTimeTracker::WriteTimeTask() {
  WriteUptimeAndRuntime();
  write_time_task_.PostDelayed(dispatcher_, write_period_);
}

void SystemTimeTracker::WriteUptimeAndRuntime() {
  rapidjson::Document doc;
  doc.SetObject();

  const int64_t uptime = zx::duration(clock_->BootNow().to_timespec()).to_msecs();
  const int64_t runtime = zx::duration(clock_->MonotonicNow().to_timespec()).to_msecs();

  doc.AddMember(kSystemTimeTrackerUptimeKey, uptime, doc.GetAllocator());
  doc.AddMember(kSystemTimeTrackerRuntimeKey, runtime, doc.GetAllocator());

  rapidjson::StringBuffer buffer;
  rapidjson::Writer<rapidjson::StringBuffer> writer(buffer);
  doc.Accept(writer);

  if (!files::WriteFileInTwoPhases(write_path_, buffer.GetString(),
                                   files::GetDirectoryName(write_path_))) {
    FX_LOGS_FIRST_N(ERROR, 10) << "Failed to write system time to: " << write_path_;
  }
}

}  // namespace forensics::feedback
