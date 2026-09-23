// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_FILE_BACKED_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_FILE_BACKED_PROVIDER_H_

#include <lib/fpromise/promise.h>

#include <string>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/utils/errors.h"

namespace forensics::feedback {

// Returns a file's content as an attachment or an error indicating why the content is missing.
class FileBackedProvider : public AttachmentProvider {
 public:
  explicit FileBackedProvider(std::string path, bool warn_if_unavailable = true);

  // Returns an immediately ready promise containing the file's content.
  ::fpromise::promise<AttachmentData> Get(uint64_t ticket) override;

  // No-op because collection happens synchronously
  void ForceCompletion(uint64_t ticket, Error error) override;

 private:
  std::string path_;
  bool warn_if_unavailable_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_FILE_BACKED_PROVIDER_H_
