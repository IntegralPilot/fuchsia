// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_TESTS_STUB_ATTACHMENT_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_TESTS_STUB_ATTACHMENT_PROVIDER_H_

#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <stdint.h>

#include <map>
#include <string>
#include <variant>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace forensics::feedback_data {

class StubAttachmentProvider : public feedback::AttachmentProvider {
 public:
  explicit StubAttachmentProvider(std::string timeout_value);

  // Returns a promise to the data and allows collection to be terminated early with |ticket|.
  fpromise::promise<feedback::AttachmentData> Get(uint64_t ticket) override;

  // Completes *all* data collection promises successfully using a value of |success_value|, if it
  // hasn't already completed.
  void CompleteSuccessfully(std::string success_value);

  // Completes the data collection promise associated with |ticket| early, if it hasn't
  // already completed.
  void ForceCompletion(uint64_t ticket, Error error) override;

 private:
  const std::string timeout_value_;
  std::map<uint64_t, fit::callback<void(std::variant<std::string, Error>)>> completers_;

  fxl::WeakPtrFactory<StubAttachmentProvider> ptr_factory_{this};
};

}  // namespace forensics::feedback_data

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_TESTS_STUB_ATTACHMENT_PROVIDER_H_
