// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DATA_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DATA_PROVIDER_H_

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <fidl/fuchsia.feedback/cpp/test_base.h>
#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include <cstdlib>
#include <map>
#include <memory>
#include <string>

#include "src/developer/forensics/feedback_data/data_provider.h"
#include "src/developer/forensics/testing/stubs/fidl_server.h"

namespace forensics {
namespace stubs {

using DataProviderServerBase = SingleBindingFidlServer<fuchsia_feedback::DataProvider>;

class DataProviderBase : public DataProviderServerBase, public feedback_data::DataProviderInternal {
 public:
  ~DataProviderBase() override = default;
};

class DataProvider : public DataProviderBase {
 public:
  DataProvider(const std::map<std::string, std::string>& annotations,
               const std::string& snapshot_key)
      : annotations_(annotations), snapshot_key_(snapshot_key) {}

  // |fuchsia_feedback::DataProvider|
  void GetSnapshot(GetSnapshotRequest& request, GetSnapshotCompleter::Sync& completer) override;

  // |feedback::DataProviderInternal|
  void GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                           GetSnapshotInternalCallback callback) override;

 protected:
  const std::map<std::string, std::string> annotations_;
  const std::string snapshot_key_;
};

class DataProviderReturnsNoAttachment : public DataProvider {
 public:
  DataProviderReturnsNoAttachment(const std::map<std::string, std::string>& annotations)
      : DataProvider(annotations, /*snapshot_key=*/"") {}

  // |fuchsia_feedback::DataProvider|
  void GetSnapshot(GetSnapshotRequest& request, GetSnapshotCompleter::Sync& completer) override;

  // |feedback::DataProviderInternal|
  void GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                           GetSnapshotInternalCallback callback) override;
};

class DataProviderReturnsEmptySnapshot : public DataProviderBase {
 public:
  // |fuchsia_feedback::DataProvider|
  void GetSnapshot(GetSnapshotRequest& request, GetSnapshotCompleter::Sync& completer) override;

  // |feedback::DataProviderInternal|
  void GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                           GetSnapshotInternalCallback callback) override;
};

class DataProviderTracksNumCalls : public DataProviderBase {
 public:
  DataProviderTracksNumCalls(size_t expected_num_calls) : expected_num_calls_(expected_num_calls) {}
  ~DataProviderTracksNumCalls();

  // |fuchsia_feedback::DataProvider|
  void GetSnapshot(GetSnapshotRequest& request, GetSnapshotCompleter::Sync& completer) override;

  // |feedback::DataProviderInternal|
  void GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                           GetSnapshotInternalCallback callback) override;

 private:
  const size_t expected_num_calls_;

  size_t num_calls_{0};
};

class DataProviderReturnsOnDemand : public DataProviderBase {
 public:
  DataProviderReturnsOnDemand(const std::map<std::string, std::string>& annotations,
                              std::string snapshot_key)
      : annotations_(annotations), snapshot_key_(std::move(snapshot_key)) {}

  // |fuchsia_feedback::DataProvider|
  void GetSnapshot(GetSnapshotRequest& request, GetSnapshotCompleter::Sync& completer) override;

  // |feedback::DataProviderInternal|
  void GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                           GetSnapshotInternalCallback callback) override;

  // Returns the Uuids for snapshots requested through GetSnapshotInternal that have not yet been
  // popped via PopSnapshotInternalCallback.
  std::deque<std::string> GetPendingUuids();

  void PopSnapshotCallback();
  void PopSnapshotInternalCallback();

 private:
  const std::map<std::string, std::string> annotations_;
  const std::string snapshot_key_;
  std::queue<GetSnapshotCompleter::Async> snapshot_callbacks_;
  std::queue<GetSnapshotInternalCallback> snapshot_internal_callbacks_;
  std::deque<std::string> pending_uuids_;
};

class DataProviderSnapshotOnly : public DataProviderBase {
 public:
  DataProviderSnapshotOnly(fuchsia_feedback::Attachment snapshot)
      : snapshot_(std::move(snapshot)) {}

  // |fuchsia_feedback::DataProvider|
  void GetSnapshot(GetSnapshotRequest& request, GetSnapshotCompleter::Sync& completer) override;

  // |feedback::DataProviderInternal|
  void GetSnapshotInternal(zx::duration timeout, const std::string& uuid,
                           GetSnapshotInternalCallback callback) override;

 private:
  fuchsia_feedback::Attachment snapshot_;
};

}  // namespace stubs
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_STUBS_DATA_PROVIDER_H_
