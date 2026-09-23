// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/utc_time_provider.h"

#include <lib/fit/function.h>

#include <optional>

#include "src/developer/forensics/utils/time.h"
#include "src/lib/files/file.h"

namespace forensics {

UtcTimeProvider::UtcTimeProvider(UtcClockReadyWatcherBase* utc_clock_ready_watcher,
                                 timekeeper::Clock* clock)
    : UtcTimeProvider(utc_clock_ready_watcher, clock, std::nullopt) {}

UtcTimeProvider::UtcTimeProvider(UtcClockReadyWatcherBase* utc_clock_ready_watcher,
                                 timekeeper::Clock* clock,
                                 PreviousBootFile utc_boot_difference_file)
    : UtcTimeProvider(utc_clock_ready_watcher, clock, std::optional(utc_boot_difference_file)) {}

UtcTimeProvider::UtcTimeProvider(UtcClockReadyWatcherBase* utc_clock_ready_watcher,
                                 timekeeper::Clock* clock,
                                 std::optional<PreviousBootFile> utc_boot_difference_file)
    : clock_(clock),
      utc_boot_difference_file_(std::move(utc_boot_difference_file)),
      previous_boot_utc_boot_difference_(std::nullopt),
      utc_clock_ready_watcher_(utc_clock_ready_watcher) {
  utc_clock_ready_watcher->OnClockReady(
      fit::bind_member<&UtcTimeProvider::OnClockLoggingQuality>(this));

  if (!utc_boot_difference_file_.has_value()) {
    return;
  }

  std::string buf;
  if (!files::ReadFileToString(utc_boot_difference_file_.value().PreviousBootPath(), &buf)) {
    return;
  }

  previous_boot_utc_boot_difference_ = zx::duration(strtoll(buf.c_str(), nullptr, /*base*/ 10));
}

std::optional<timekeeper::time_utc> UtcTimeProvider::CurrentTime() const {
  if (!utc_clock_ready_watcher_->IsUtcClockReady()) {
    return std::nullopt;
  }

  return CurrentUtcTimeRaw(clock_);
}

std::optional<zx::duration> UtcTimeProvider::CurrentUtcBootDifference() const {
  if (!utc_clock_ready_watcher_->IsUtcClockReady()) {
    return std::nullopt;
  }

  const timekeeper::time_utc current_utc_time = CurrentUtcTimeRaw(clock_);

  const zx::duration utc_boot_difference(current_utc_time.get() - clock_->BootNow().get());
  if (utc_boot_difference_file_.has_value()) {
    // Write the most recent UTC-boot difference in case either clock has been adjusted.
    files::WriteFile(utc_boot_difference_file_.value().CurrentBootPath(),
                     std::to_string(utc_boot_difference.get()));
  }
  return utc_boot_difference;
}

std::optional<zx::duration> UtcTimeProvider::PreviousBootUtcBootDifference() const {
  return previous_boot_utc_boot_difference_;
}

void UtcTimeProvider::OnClockLoggingQuality() {
  // Write the current difference between the UTC and boot clocks.
  if (utc_boot_difference_file_.has_value()) {
    const timekeeper::time_utc current_utc_time = CurrentUtcTimeRaw(clock_);
    const zx::duration utc_boot_difference(current_utc_time.get() - clock_->BootNow().get());
    files::WriteFile(utc_boot_difference_file_.value().CurrentBootPath(),
                     std::to_string(utc_boot_difference.get()));
  }
}

}  // namespace forensics
