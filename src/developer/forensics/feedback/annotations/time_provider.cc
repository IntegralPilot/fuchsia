// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/time_provider.h"

#include <fuchsia/time/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <string>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/time.h"

namespace forensics::feedback {
namespace {

ErrorOrString GetFormattedDuration(zx::duration duration, std::string_view error_source) {
  const std::optional<std::string> formatted_duration = FormatDuration(duration);
  if (!formatted_duration) {
    FX_LOGS(ERROR) << "Got negative value from '" << error_source << "'";
    return ErrorOrString(Error::kBadValue);
  }

  return ErrorOrString(*formatted_duration);
}

}  // namespace

TimeProvider::TimeProvider(async_dispatcher_t* dispatcher, zx::unowned_clock clock_handle,
                           std::unique_ptr<timekeeper::Clock> clock)
    : clock_(std::move(clock)),
      wait_for_logging_quality_clock_(this, clock_handle->get_handle(),
                                      fuchsia::time::SIGNAL_UTC_CLOCK_LOGGING_QUALITY,
                                      /*options=*/0) {
  if (const zx_status_t status = wait_for_logging_quality_clock_.Begin(dispatcher);
      status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to wait for logging quality clock";
  }
}

std::set<std::string> TimeProvider::GetAnnotationKeys() {
  return {
      kDeviceRuntimeKey,
      kDeviceTotalSuspendedTimeKey,
      kDeviceUptimeKey,
      kDeviceUtcTimeKey,
  };
}

std::set<std::string> TimeProvider::GetKeys() const { return TimeProvider::GetAnnotationKeys(); }

Annotations TimeProvider::Get() {
  const auto utc_time = [this]() -> ErrorOrString {
    if (is_utc_time_accurate_) {
      return ErrorOrString(CurrentUtcTime(clock_.get()));
    }

    return ErrorOrString(Error::kMissingValue);
  }();

  const zx::duration monotonic_now = zx::duration(clock_->MonotonicNow().to_timespec());
  const zx::duration boot_now = zx::duration(clock_->BootNow().to_timespec());
  const zx::duration time_suspended = boot_now - monotonic_now;

  return {
      {kDeviceRuntimeKey, GetFormattedDuration(monotonic_now, "timekeeper::Clock::MonotonicNow()")},
      {kDeviceTotalSuspendedTimeKey,
       GetFormattedDuration(time_suspended,
                            "timekeeper::Clock::BootNow() - timekeeper::Clock::MonotonicNow()")},
      {kDeviceUptimeKey, GetFormattedDuration(boot_now, "timekeeper::Clock::BootNow()")},
      {kDeviceUtcTimeKey, utc_time},
  };
}

void TimeProvider::OnClockLoggingQuality(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                         zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status)
        << "Wait for logging quality clock completed with error, trying again";

    // Attempt to wait for the clock to achieve logging quality again.
    wait->Begin(dispatcher);
    return;
  }

  FX_LOGS(INFO) << "Received signal that UTC clock is accurate";
  is_utc_time_accurate_ = true;
}

}  // namespace forensics::feedback
