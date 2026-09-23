// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DIAGNOSTICS_ARCHIVE_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DIAGNOSTICS_ARCHIVE_H_

#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/fuchsia.diagnostics/cpp/test_base.h>
#include <lib/async/dispatcher.h>

#include "src/developer/forensics/testing/stubs/diagnostics_batch_iterator.h"
#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics {
namespace stubs {

using DiagnosticsArchiveBase = SingleBindingFidlServer<fuchsia_diagnostics::ArchiveAccessor>;

class DiagnosticsArchive : public DiagnosticsArchiveBase {
 public:
  DiagnosticsArchive(async_dispatcher_t* dispatcher,
                     std::unique_ptr<DiagnosticsBatchIteratorBase> batch_iterator)
      : dispatcher_(dispatcher), batch_iterator_(std::move(batch_iterator)) {}

  // |fuchsia_diagnostics::ArchiveAccessor|
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override;

 protected:
  async_dispatcher_t* dispatcher() const { return dispatcher_; }
  std::unique_ptr<DiagnosticsBatchIteratorBase>& BatchIterator() { return batch_iterator_; }

 private:
  async_dispatcher_t* dispatcher_;
  std::unique_ptr<DiagnosticsBatchIteratorBase> batch_iterator_;
};

class DiagnosticsArchiveSequentialIterators : public DiagnosticsArchiveBase {
 public:
  DiagnosticsArchiveSequentialIterators(
      async_dispatcher_t* dispatcher,
      std::vector<std::unique_ptr<DiagnosticsBatchIteratorBase>> batch_iterators)
      : dispatcher_(dispatcher),
        batch_iterators_(std::move(batch_iterators)),
        current_iterator_index_(0) {}

  // |fuchsia_diagnostics::ArchiveAccessor|
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override;

 private:
  async_dispatcher_t* dispatcher_;
  std::vector<std::unique_ptr<DiagnosticsBatchIteratorBase>> batch_iterators_;
  size_t current_iterator_index_;
};

class DiagnosticsArchiveCaptureParameters : public DiagnosticsArchiveBase {
 public:
  explicit DiagnosticsArchiveCaptureParameters(fuchsia_diagnostics::StreamParameters* parameters)
      : parameters_(parameters) {}

  // |fuchsia_diagnostics::ArchiveAccessor|
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override {
    *parameters_ = std::move(request.stream_parameters());
  }

 private:
  // Not owned
  fuchsia_diagnostics::StreamParameters* parameters_;
};

class DiagnosticsArchiveClosesArchiveConnection : public DiagnosticsArchiveBase {
 public:
  // |fuchsia_diagnostics::ArchiveAccessor|
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override {
    CloseConnection(ZX_ERR_PEER_CLOSED);
  }
};

class DiagnosticsArchiveClosesIteratorConnection : public DiagnosticsArchiveBase {
 public:
  // |fuchsia_diagnostics::ArchiveAccessor|
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override;
};

class DiagnosticsArchiveClosesFirstIteratorConnection : public DiagnosticsArchive {
 public:
  using DiagnosticsArchive::DiagnosticsArchive;

  // |fuchsia_diagnostics::ArchiveAccessor|
  void StreamDiagnostics(StreamDiagnosticsRequest& request,
                         StreamDiagnosticsCompleter::Sync& completer) override;

 private:
  bool is_first_{true};
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DIAGNOSTICS_ARCHIVE_H_
