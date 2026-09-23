// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DIAGNOSTICS_READER_CPP_LOGS_H_
#define LIB_DIAGNOSTICS_READER_CPP_LOGS_H_

#include <fidl/fuchsia.diagnostics.types/cpp/fidl.h>
#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/stdcompat/optional.h>

#include <cstdint>
#include <deque>
#include <optional>

#include <rapidjson/document.h>

namespace diagnostics::reader {

// Container for inspect data returned by a component.
//
// This class provides methods for parsing common fields from diagnostics output.
class LogsData {
 public:
  struct DroppedLogsError {
    uint64_t count;
  };

  struct RolledOutLogsError {
    uint64_t count;
  };

  struct FailedToParseRecordError {
    std::string message;
  };

  struct OtherError {
    std::string message;
  };

  using Error =
      std::variant<DroppedLogsError, RolledOutLogsError, FailedToParseRecordError, OtherError>;

  struct Metadata final {
    std::string component_url;
    uint64_t timestamp;
    fuchsia_diagnostics_types::Severity severity;
    std::vector<std::string> tags;
    std::optional<uint64_t> pid;
    std::optional<uint64_t> tid;
    std::optional<std::string> file;
    std::optional<uint64_t> line;
    std::vector<Error> errors;
  };

  // Create a new LogsData wrapper from a JSON document.
  explicit LogsData(rapidjson::Document document);

  // Movable but not copyable.
  LogsData(const LogsData&) = delete;
  LogsData(LogsData&&) = default;
  LogsData& operator=(const LogsData&) = delete;
  LogsData& operator=(LogsData&&) = default;

  // Return the moniker of the component that created this data.
  const std::string& moniker() const { return moniker_; }

  // Return the version of the component that created this data.
  uint64_t version() const { return version_; }

  // Return the metadata of the component that created this data.
  const LogsData::Metadata& metadata() const { return metadata_; }

  // Return the message of the log.
  const std::string& message() const { return message_; }

  // Return the structured keys of the log.
  const std::vector<inspect::PropertyValue>& keys() const { return keys_; }

 private:
  // Moniker of the component that generated the payload.
  std::string moniker_;

  // The metadata for the diagnostics payload.
  Metadata metadata_;

  // The message of this log.
  std::string message_;

  /// The structured keys in the message and their values.
  std::vector<inspect::PropertyValue> keys_;

  // Schema version.
  uint64_t version_;

  void LoadArray(const std::string& name, const rapidjson::Value::Array& arr);
};

class LogsSubscription {
 public:
  using Promise = fpromise::promise<std::optional<LogsData>, std::string>;

  explicit LogsSubscription(fidl::SharedClient<fuchsia_diagnostics::BatchIterator> iterator,
                            async::Executor& executor);

  // Not movable nor copyable.
  LogsSubscription(const LogsSubscription&) = delete;
  LogsSubscription(LogsSubscription&&) = delete;
  LogsSubscription& operator=(const LogsSubscription&) = delete;
  LogsSubscription& operator=(LogsSubscription&&) = delete;

  /// Returns a promise that will resolve when the iterator receives the next
  /// log. When the stream has completed and no more data will come, returns `std::nullopt`.
  LogsSubscription::Promise Next();

  /// Whether or not the subscription is Done. When this returns `true`,  `Next` is guaranteed to
  /// return `std::nullopt`.
  bool Done();

 private:
  LogsSubscription::Promise ReadBatch();
  static std::optional<LogsData> LoadJson(rapidjson::Document document,
                                          std::shared_ptr<std::deque<LogsData>> pending,
                                          std::shared_ptr<bool> done);

  // Iterator connection.
  fidl::SharedClient<fuchsia_diagnostics::BatchIterator> iterator_;

  // Pending data to return before calling BatchIterator/GetNext again.
  std::shared_ptr<std::deque<LogsData>> pending_;
  // The scope to tie async task lifetimes to this object.
  fpromise::scope scope_;
  // Whether or not this subscription has completed and will return more data.
  std::shared_ptr<bool> done_;
};

}  // namespace diagnostics::reader

#endif  // LIB_DIAGNOSTICS_READER_CPP_LOGS_H_
