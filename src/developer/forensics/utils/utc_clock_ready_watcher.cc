// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/utc_clock_ready_watcher.h"

#include <fuchsia/time/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

namespace forensics {

UtcClockReadyWatcher::UtcClockReadyWatcher(async_dispatcher_t* dispatcher,
                                           zx::unowned_clock clock_handle)
    : wait_for_logging_quality_clock_(this, clock_handle->get_handle(),
                                      fuchsia::time::SIGNAL_UTC_CLOCK_LOGGING_QUALITY,
                                      /*options=*/0) {
  if (const zx_status_t status = wait_for_logging_quality_clock_.Begin(dispatcher);
      status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to wait for logging quality clock";
  }
}

void UtcClockReadyWatcher::OnClockReady(::fit::callback<void()> callback) {
  if (is_utc_clock_ready_) {
    callback();
    return;
  }

  callbacks_.push_back(std::move(callback));
}

bool UtcClockReadyWatcher::IsUtcClockReady() const { return is_utc_clock_ready_; }

void UtcClockReadyWatcher::OnClockLoggingQuality(async_dispatcher_t* dispatcher,
                                                 async::WaitBase* wait, zx_status_t status,
                                                 const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status)
        << "Wait for logging quality clock completed with error, trying again";

    // Attempt to wait for the clock to achieve logging quality again.
    wait->Begin(dispatcher);
    return;
  }

  // |is_utc_clock_ready_| must be set to true before callbacks are run in case
  // any of them use IsUtcClockReady.
  is_utc_clock_ready_ = true;

  for (::fit::callback<void()>& callback : callbacks_) {
    callback();
  }
  callbacks_.clear();
}

}  // namespace forensics
