// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/previous_boot_log.h"

#include <lib/async/cpp/task.h>
#include <lib/fpromise/promise.h>

#include <string>

#include "src/lib/files/path.h"

namespace forensics::feedback {

PreviousBootLog::PreviousBootLog(async_dispatcher_t* dispatcher, timekeeper::Clock* clock,
                                 const std::optional<zx::duration> delete_previous_boot_log_at,
                                 std::string path)
    : FileBackedProvider(path),
      dispatcher_(dispatcher),
      clock_(clock),
      is_file_deleted_(false),
      path_(std::move(path)) {
  if (!delete_previous_boot_log_at.has_value()) {
    return;
  }

  delete_previous_boot_log_at_ = zx::time_boot(0) + *delete_previous_boot_log_at;

  // The previous boot logs will eventually be lazily deleted, but we should still try to
  // proactively delete them to save space.
  fxl::WeakPtr<PreviousBootLog> self = weak_factory_.GetWeakPtr();
  async::PostDelayedTask(
      dispatcher_,
      [self] {
        if (self && !self->is_file_deleted_) {
          FX_LOGS(INFO) << "Deleting previous boot logs after 24 hours of device runtime";
          self->is_file_deleted_ = true;
          files::DeletePath(self->path_, /*recursive=*/true);
        }
      },
      // The previous boot logs are proactively deleted after |delete_previous_boot_log_at| of
      // device runtime, not component runtime.
      zx::time_monotonic(0) + *delete_previous_boot_log_at - clock_->MonotonicNow());
}

::fpromise::promise<AttachmentData> PreviousBootLog::Get(const uint64_t ticket) {
  // Lazily check if the previous boot log should be deleted because we have no easy way to schedule
  // async work based on the boot clock.
  if (!is_file_deleted_ && delete_previous_boot_log_at_.has_value() &&
      (clock_->BootNow() >= *delete_previous_boot_log_at_)) {
    FX_LOGS(INFO) << "Lazily deleting previous boot logs after 24 hours of device uptime";
    is_file_deleted_ = true;
    files::DeletePath(path_, /*recursive=*/true);
  }

  if (is_file_deleted_) {
    return fpromise::make_ok_promise(AttachmentData(Error::kCustom));
  }

  return FileBackedProvider::Get(ticket);
}

}  // namespace forensics::feedback
