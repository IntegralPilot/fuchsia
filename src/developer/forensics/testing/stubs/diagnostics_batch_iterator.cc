// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/stubs/diagnostics_batch_iterator.h"

#include <lib/async/cpp/task.h>
#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>

#include "src/lib/fsl/vmo/strings.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace stubs {
namespace {

std::vector<fuchsia_diagnostics::FormattedContent> ToVmo(
    const std::vector<std::string>& json_batch) {
  std::vector<fuchsia_diagnostics::FormattedContent> json_batch_vmo;
  json_batch_vmo.reserve(json_batch.size());
  for (const std::string& json_chunk : json_batch) {
    fsl::SizedVmo vmo;
    FX_CHECK(fsl::VmoFromString(json_chunk, &vmo));
    fuchsia_mem::Buffer buffer(std::move(vmo.vmo()), vmo.size());
    json_batch_vmo.push_back(fuchsia_diagnostics::FormattedContent::WithJson(std::move(buffer)));
  }
  return json_batch_vmo;
}

}  // namespace

DiagnosticsBatchIterator::~DiagnosticsBatchIterator() {
  if (strict_) {
    FX_CHECK(!ExpectCall()) << fxl::StringPrintf(
        "Expected %ld more calls to GetNext() (%ld/%lu calls made)",
        std::distance(next_json_batch_, json_batches_.cend()),
        std::distance(json_batches_.cbegin(), next_json_batch_), json_batches_.size());
  }
}

void DiagnosticsBatchIterator::GetNext(GetNextCompleter::Sync& completer) {
  FX_CHECK(ExpectCall()) << fxl::StringPrintf(
      "No more calls to GetNext() expected (%lu/%lu calls made)",
      std::distance(json_batches_.cbegin(), next_json_batch_), json_batches_.size());

  completer.Reply(fit::ok(ToVmo(*next_json_batch_++)));
}

void DiagnosticsBatchIteratorNeverRespondsAfterOneBatch::GetNext(
    GetNextCompleter::Sync& completer) {
  if (has_returned_batch_) {
    completers_.push_back(completer.ToAsync());
    return;
  }

  completer.Reply(fit::ok(ToVmo(json_batch_)));
  has_returned_batch_ = true;
}

void DiagnosticsBatchIteratorReturnsError::GetNext(GetNextCompleter::Sync& completer) {
  if (!returned_error_) {
    completer.Reply(fit::error(fuchsia_diagnostics::ReaderError::kIo));
    returned_error_ = true;
  } else {
    completer.Reply(fit::ok(ToVmo({})));
  }
}

void DiagnosticsBatchIteratorDelayedBatches::GetNext(GetNextCompleter::Sync& completer) {
  async::PostDelayedTask(
      dispatcher_,
      [this, completer = completer.ToAsync()]() mutable {
        FX_CHECK(ExpectCall()) << fxl::StringPrintf(
            "No more calls to GetNext() expected (%lu/%lu calls made)",
            std::distance(json_batches_.cbegin(), next_json_batch_), json_batches_.size());

        completer.Reply(fit::ok(ToVmo(*next_json_batch_++)));
      },
      is_initial_delay_ ? initial_delay_ : delay_between_batches_);
  is_initial_delay_ = false;
}

}  // namespace stubs
}  // namespace forensics
