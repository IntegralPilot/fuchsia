// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DIAGNOSTICS_READER_CPP_INSPECT_H_
#define LIB_DIAGNOSTICS_READER_CPP_INSPECT_H_

#include <fidl/fuchsia.diagnostics/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/stdcompat/optional.h>

#include <cstdint>
#include <optional>
#include <string>

#include <rapidjson/document.h>

namespace diagnostics::reader {

// Container for inspect data returned by a component.
//
// This class provides methods for parsing common fields from diagnostics output.
class InspectData {
 public:
  struct InspectError final {
    std::string message;
  };
  struct InspectMetadata final {
    std::optional<std::string> filename = {};
    std::optional<std::string> name = {};
    std::optional<std::string> component_url;
    uint64_t timestamp;
    std::optional<std::vector<InspectError>> errors;
  };

  // Create a new InspectData wrapper around a JSON document.
  explicit InspectData(rapidjson::Document document);

  // Movable but not copyable.
  InspectData(const InspectData&) = delete;
  InspectData(InspectData&&) = default;
  InspectData& operator=(const InspectData&) = delete;
  InspectData& operator=(InspectData&&) = default;

  // Return the moniker of the component that created this data.
  const std::string& moniker() const { return moniker_; }

  // Return the version of the component that created this data.
  uint64_t version() const { return version_; }

  // Return the metadata of the component that created this data.
  const InspectData::InspectMetadata& metadata() const { return metadata_; }

  // Return the payload of the component that created this data, if defined.
  std::optional<const inspect::Hierarchy*> payload() const {
    return payload_.name().empty() ? std::nullopt : std::make_optional(&payload_);
  }

  // Return the content of the diagnostics data as a JSON value.
  const rapidjson::Value& content() const;

  // TODO(https://fxbug.dev/42158053): Switch users to use payload() and remove GetByPath()
  // Returns the value at the given path in the data contents.
  const rapidjson::Value& GetByPath(const std::vector<std::string>& path) const;

  // Returns the data formatted in json.
  std::string PrettyJson();

  // Sort properties and children of this node by name, and recursively sort each child.
  //
  // This method imposes a canonical ordering on every child value in the hierarchy for purposes of
  // comparison and output. It does not optimize operations in any way.
  //
  // The sorting rule for each of children and property values is as follows:
  // - If and only if all names match non-negative integral strings, sort numerically.
  // - Otherwise, sort lexicographically.
  void Sort();

  inspect::Hierarchy&& TakePayload() { return std::move(payload_); }

 private:
  // The document wrapped by this container.
  rapidjson::Document document_;

  // Moniker of the component that generated the payload.
  std::string moniker_;

  // The metadata for the diagnostics payload.
  InspectMetadata metadata_;

  // Payload containing diagnostics data, if the payload exists, else empty.
  inspect::Hierarchy payload_;

  // Schema version.
  uint64_t version_;
};

}  // namespace diagnostics::reader

#endif  // LIB_DIAGNOSTICS_READER_CPP_INSPECT_H_
