// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PREVIOUS_BOOT_LOG_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PREVIOUS_BOOT_LOG_H_

#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>

#include <optional>
#include <string>

#include "src/developer/forensics/feedback/attachments/file_backed_provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics::feedback {

// Collects the previous boot log and proactively deletes the previous boot log after
// |delete_previous_boot_log_at| of device runtime. When the previous boot log is collected, the
// previous boot log will also be lazily deleted if more than |delete_previous_boot_log_at| of
// device uptime has passed.
class PreviousBootLog : public FileBackedProvider {
 public:
  PreviousBootLog(async_dispatcher_t* dispatcher, timekeeper::Clock* clock,
                  std::optional<zx::duration> delete_previous_boot_log_at, std::string path);

  // Returns a promise, that is immediately available, to the previous boot log.
  ::fpromise::promise<AttachmentData> Get(uint64_t ticket) override;

 private:
  async_dispatcher_t* dispatcher_;

  timekeeper::Clock* clock_;
  bool is_file_deleted_;
  std::string path_;
  std::optional<zx::time_boot> delete_previous_boot_log_at_;
  fxl::WeakPtrFactory<PreviousBootLog> weak_factory_{this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PREVIOUS_BOOT_LOG_H_
