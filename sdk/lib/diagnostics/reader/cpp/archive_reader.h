// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DIAGNOSTICS_READER_CPP_ARCHIVE_READER_H_
#define LIB_DIAGNOSTICS_READER_CPP_ARCHIVE_READER_H_

#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <fidl/fuchsia.diagnostics/cpp/markers.h>
#include <lib/async/cpp/executor.h>
#include <lib/diagnostics/reader/cpp/inspect.h>
#include <lib/diagnostics/reader/cpp/logs.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/stdcompat/optional.h>

#include <cstdint>
#include <list>
#include <optional>

#include <rapidjson/document.h>

namespace diagnostics::reader {

// ArchiveReader supports reading Inspect data from an Archive.
class ArchiveReader {
 public:
  // Create a new ArchiveReader.
  ArchiveReader(async_dispatcher_t* dispatcher, std::vector<std::string> selectors);
  explicit ArchiveReader(async_dispatcher_t* dispatcher) : ArchiveReader(dispatcher, {}) {}

  // Creates a new ArchiveReader using the specified client.
  ArchiveReader(async_dispatcher_t* dispatcher, std::vector<std::string> selectors,
                fidl::ClientEnd<fuchsia_diagnostics::ArchiveAccessor> archive);

  // Get a snapshot of the Inspect data at the current point in time.
  //
  // Returns an error if the ArchiveAccessorPtr is not bound.
  fpromise::promise<std::vector<InspectData>, std::string> GetInspectSnapshot();

  // Set the ArchiveReader's selectors to this exact list, overwriting any previously supplied.
  ArchiveReader& SetSelectors(std::vector<std::string> selectors) {
    selectors_ = std::move(selectors);
    return *this;
  }

  // Add these selectors to the ArchiveReader's current set of selectors.
  ArchiveReader& AddSelectors(std::vector<std::string> selectors) {
    selectors_.insert(selectors_.end(), selectors.begin(), selectors.end());
    return *this;
  }

  // Gets a snapshot of the Inspect data at the point in time in which all listed component
  // names are present.
  //
  // Returns an error if the ArchiveAccessorPtr is not bound.
  fpromise::promise<std::vector<InspectData>, std::string> SnapshotInspectUntilPresent(
      std::vector<std::string> component_names);

  // Subscribes to logs using the given `mode`.
  LogsSubscription GetLogs(fuchsia_diagnostics::StreamMode mode);

 private:
  void InnerSnapshotInspectUntilPresent(
      fpromise::completer<std::vector<InspectData>, std::string> bridge,
      std::vector<std::string> component_names);

  fidl::SharedClient<fuchsia_diagnostics::BatchIterator> GetBatchIterator(
      fuchsia_diagnostics::DataType data_type, fuchsia_diagnostics::StreamMode stream_mode);

  fidl::SharedClient<fuchsia_diagnostics::ArchiveAccessor> Bind(
      async_dispatcher_t* dispatcher,
      fidl::ClientEnd<fuchsia_diagnostics::ArchiveAccessor> archive);

  // Resolved archive if present.
  fidl::SharedClient<fuchsia_diagnostics::ArchiveAccessor> archive_;

  // The executor on which promise continuations run.
  async::Executor executor_;

  // The selectors used to filter data streamed from this reader.
  std::vector<std::string> selectors_;

  // The scope to tie async task lifetimes to this object.
  fpromise::scope scope_;
};

void EmplaceInspect(rapidjson::Document document, std::vector<InspectData>* out);

// Construct a selector from the provided values:
// 1) moniker: this should be an unsanitized moniker like "core/simple" or "core/less:simple".
// 2) inspect_tree_names: if nullopt, uses the default tree name. If a list of names, puts them
//                        in the name filter list. If "...", constructs a filter list that matches
//                        any inspect tree name.
// 3) hierarchy: the node path hierarchy. These should be sanitized, so asterisks and colons in
//               the values need to be escaped (ie "\\*" or "\\:"). If an empty vector is provided,
//               the root node will be inserted. If a non-empty vector is provided, note that it
//               should start at the root node.
// 4) property: the property to select. If nullopt, no property is inserted (ie the final node
//              in `hierarchy` is used).
std::string MakeSelector(std::string_view moniker,
                         std::optional<std::vector<std::string>> inspect_tree_names,
                         std::vector<std::string> hierarchy, std::optional<std::string> property);

std::string SanitizeMonikerForSelectors(std::string_view moniker);

}  // namespace diagnostics::reader

#endif  // LIB_DIAGNOSTICS_READER_CPP_ARCHIVE_READER_H_
