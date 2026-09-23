// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PREVIOUS_BOOT_INSPECT_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PREVIOUS_BOOT_INSPECT_H_

#include <fuchsia/diagnostics/persistence/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/sys/cpp/service_directory.h>

#include <map>
#include <memory>
#include <optional>
#include <string>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/backoff/backoff.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace forensics::feedback {

// Collects the Inspect data from the previous boot via
// fuchsia.diagnostics.persistence.PreviousBootDataProvider and caches it in a file on disk.
class PreviousBootInspect : public AttachmentProvider {
 public:
  PreviousBootInspect(async_dispatcher_t* dispatcher,
                      std::shared_ptr<sys::ServiceDirectory> services,
                      std::unique_ptr<backoff::Backoff> backoff, RedactorBase* redactor,
                      std::string path);

  // Returns a promise to the previous boot inspect data and allows collection to be terminated
  // early with |ticket|.
  ::fpromise::promise<AttachmentData> Get(uint64_t ticket) override;

  // Completes the inspect data collection promise associated with |ticket| early, if it hasn't
  // already completed.
  void ForceCompletion(uint64_t ticket, Error error) override;

 private:
  void WatchPreviousBootData(async_dispatcher_t* dispatcher,
                             std::shared_ptr<sys::ServiceDirectory> services);
  void OnDataReceived(fuchsia::diagnostics::persistence::PreviousBootData data);
  void OnError();

  AttachmentData ReadAttachmentData() const;

  std::unique_ptr<backoff::Backoff> backoff_;
  RedactorBase* redactor_;
  std::string path_;

  fuchsia::diagnostics::persistence::PreviousBootDataProviderPtr data_provider_;

  std::optional<Error> error_;
  std::map<uint64_t, ::fit::callback<void(AttachmentData)>> completers_;

  fxl::WeakPtrFactory<PreviousBootInspect> ptr_factory_{this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_PREVIOUS_BOOT_INSPECT_H_
