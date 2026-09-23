// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DIAGNOSTICS_BATCH_ITERATOR_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DIAGNOSTICS_BATCH_ITERATOR_H_

#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/fuchsia.diagnostics/cpp/test_base.h>

#include <string>
#include <vector>

#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics {
namespace stubs {

using DiagnosticsBatchIteratorBase = SingleBindingFidlServer<fuchsia_diagnostics::BatchIterator>;

class DiagnosticsBatchIterator : public DiagnosticsBatchIteratorBase {
 public:
  DiagnosticsBatchIterator() : json_batches_({}), strict_(true) {}
  explicit DiagnosticsBatchIterator(const std::vector<std::vector<std::string>>& json_batches,
                                    bool strict = true)
      : json_batches_(json_batches), strict_(strict) {
    next_json_batch_ = json_batches_.cbegin();
  }

  ~DiagnosticsBatchIterator() override;

  // Whether the batch iterator expects at least one more call to GetNext().
  bool ExpectCall() { return next_json_batch_ != json_batches_.cend(); }

  void GetNext(GetNextCompleter::Sync& completer) override;

 protected:
  const std::vector<std::vector<std::string>> json_batches_;
  decltype(json_batches_)::const_iterator next_json_batch_;

 private:
  const bool strict_;
};

class DiagnosticsBatchIteratorNeverRespondsAfterOneBatch : public DiagnosticsBatchIteratorBase {
 public:
  DiagnosticsBatchIteratorNeverRespondsAfterOneBatch(const std::vector<std::string>& json_batch)
      : json_batch_(json_batch) {}

  // |fuchsia_diagnostics::BatchIterator|
  void GetNext(GetNextCompleter::Sync& completer) override;

 private:
  const std::vector<std::string> json_batch_;
  bool has_returned_batch_ = false;
  std::vector<GetNextCompleter::Async> completers_;
};

class DiagnosticsBatchIteratorNeverResponds : public DiagnosticsBatchIteratorBase {
 public:
  // |fuchsia_diagnostics::BatchIterator|
  void GetNext(GetNextCompleter::Sync& completer) override {
    completers_.push_back(completer.ToAsync());
  }

 private:
  std::vector<GetNextCompleter::Async> completers_;
};

class DiagnosticsBatchIteratorReturnsError : public DiagnosticsBatchIteratorBase {
 public:
  DiagnosticsBatchIteratorReturnsError() {}

  // |fuchsia_diagnostics::BatchIterator|
  void GetNext(GetNextCompleter::Sync& completer) override;

 private:
  bool returned_error_{false};
};

class DiagnosticsBatchIteratorDelayedBatches : public DiagnosticsBatchIterator {
 public:
  DiagnosticsBatchIteratorDelayedBatches(async_dispatcher_t* dispatcher,
                                         const std::vector<std::vector<std::string>>& json_batches,
                                         zx::duration initial_delay,
                                         zx::duration delay_between_batches, bool strict = true)
      : DiagnosticsBatchIterator(json_batches, strict),
        dispatcher_(dispatcher),
        initial_delay_(initial_delay),
        delay_between_batches_(delay_between_batches) {}

  void GetNext(GetNextCompleter::Sync& completer) override;

 private:
  async_dispatcher_t* dispatcher_;
  zx::duration initial_delay_;
  zx::duration delay_between_batches_;
  bool is_initial_delay_{true};
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DIAGNOSTICS_BATCH_ITERATOR_H_
