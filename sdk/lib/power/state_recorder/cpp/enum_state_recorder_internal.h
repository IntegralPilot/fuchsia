// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_ENUM_STATE_RECORDER_INTERNAL_H_
#define LIB_POWER_STATE_RECORDER_CPP_ENUM_STATE_RECORDER_INTERNAL_H_

#include <lib/inspect/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/concepts.h>
#include <lib/power/state_recorder/cpp/inspect_buffer_internal.h>
#include <lib/trace-engine/types.h>
#include <lib/zx/process.h>
#include <zircon/compiler.h>

#include <map>
#include <memory>
#include <string>
#include <unordered_map>

namespace power_observability::internal {

// Stores a state name as both a std::string (for Inspect) and trace_string_ref_t (for trace);
struct StateName {
  // Use a unique_ptr to guarantee address stability, so the ref in `trace_name` remains valid on
  // move.
  std::unique_ptr<std::string> inspect_name;
  trace_string_ref_t trace_name;

  explicit StateName(std::string name)
      : inspect_name(std::make_unique<std::string>(name)),
        trace_name(trace_make_inline_string_ref(inspect_name->c_str(), inspect_name->length())) {}
};

// Helper class for mapping a state's enum value to its corresponding StateName.
template <typename T>
  requires IsRecordableEnumType<T>
class StateNameLookup {
 public:
  explicit StateNameLookup(std::map<T, std::string> string_names) {
    for (const auto& [state_enum, state_name] : string_names) {
      state_names_.emplace(state_enum, state_name);
    }
  }

  const StateName* GetStateName(T state_enum) const {
    static const StateName UNKNOWN_STATE_NAME = StateName("<Unknown>");

    auto it = state_names_.find(state_enum);
    if (it != state_names_.end()) {
      return &it->second;
    }
    return &UNKNOWN_STATE_NAME;
  }

 private:
  std::unordered_map<T, StateName> state_names_;
};

// Records data in an underlying TimestampedBuffer to a lazy node, mapping enum values to names.
template <typename T>
  requires IsRecordableEnumType<T>
class EnumLazyInspectRecorder : public LazyInspectRecorderBase<T> {
 public:
  // The address of this object needs to be stable for the lazy node callback, so we force it to be
  // constructed behind a unique_ptr.
  static std::unique_ptr<EnumLazyInspectRecorder> Create(
      std::shared_ptr<StateNameLookup<T>> name_lookup, size_t capacity,
      inspect::Node& parent_node) {
    // The constructor is private, so we can't use `std::make_unique`.
    return std::unique_ptr<EnumLazyInspectRecorder>(
        new EnumLazyInspectRecorder(name_lookup, capacity, parent_node));
  }

  EnumLazyInspectRecorder(std::shared_ptr<StateNameLookup<T>> name_lookup, size_t capacity,
                          inspect::Node& parent_node)
      : LazyInspectRecorderBase<T>(capacity, parent_node), name_lookup_(std::move(name_lookup)) {}

 private:
  std::shared_ptr<StateNameLookup<T>> name_lookup_;
};

}  // namespace power_observability::internal

#endif  // LIB_POWER_STATE_RECORDER_CPP_ENUM_STATE_RECORDER_INTERNAL_H_
