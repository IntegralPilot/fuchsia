// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_UTC_TIME_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_UTC_TIME_PROVIDER_H_

#include <optional>

#include "src/developer/forensics/utils/previous_boot_file.h"
#include "src/developer/forensics/utils/utc_clock_ready_watcher.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics {

// Provides the UTC time only if the device's UTC clock has achieved logging quality.
//
// Can be configured to record the UTC-boot difference from the previous boot by providing a
// non-nullopt |utc_boot_difference_path|.
class UtcTimeProvider {
 public:
  UtcTimeProvider(UtcClockReadyWatcherBase* utc_clock_ready_watcher, timekeeper::Clock* clock);
  UtcTimeProvider(UtcClockReadyWatcherBase* utc_clock_ready_watcher, timekeeper::Clock* clock,
                  PreviousBootFile utc_boot_difference_file);

  // Returns the current UTC time if the device's UTC time is accurate, std::nullopt otherwise.
  std::optional<timekeeper::time_utc> CurrentTime() const;

  // Returns the difference between the UTC clock and the device's boot time if the device's UTC
  // time is accurate, std::nullopt otherwise.
  //
  // This value can be added to a boot time to convert it to a UTC time.
  std::optional<zx::duration> CurrentUtcBootDifference() const;
  std::optional<zx::duration> PreviousBootUtcBootDifference() const;

 private:
  UtcTimeProvider(UtcClockReadyWatcherBase* utc_clock_ready_watcher, timekeeper::Clock* clock,
                  std::optional<PreviousBootFile> utc_boot_difference_file);

  // Keep waiting on the clock handle until the clock has achieved logging quality.
  void OnClockLoggingQuality();

  timekeeper::Clock* clock_;

  std::optional<PreviousBootFile> utc_boot_difference_file_;

  // The last difference between the UTC and boot clocks in the previous boot.
  std::optional<zx::duration> previous_boot_utc_boot_difference_;

  UtcClockReadyWatcherBase* utc_clock_ready_watcher_;
};

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_UTC_TIME_PROVIDER_H_
