// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_NUMERIC_STATE_RECORDER_INTERNAL_H_
#define LIB_POWER_STATE_RECORDER_CPP_NUMERIC_STATE_RECORDER_INTERNAL_H_

#include <lib/inspect/cpp/inspect.h>
#include <lib/power/state_recorder/cpp/concepts.h>
#include <lib/power/state_recorder/cpp/inspect_buffer_internal.h>

#include <memory>

namespace power_observability::internal {

template <typename T>
  requires IsRecordableNumericType<T>
class NumericLazyInspectRecorder : public LazyInspectRecorderBase<T> {
 public:
  // The address of this object needs to be stable for the lazy node callback, so we force it to be
  // constructed behind a unique_ptr.
  static std::unique_ptr<NumericLazyInspectRecorder> Create(size_t capacity,
                                                            inspect::Node& parent_node) {
    // The constructor is private, so we can't use `std::make_unique`.
    return std::unique_ptr<NumericLazyInspectRecorder>(
        new NumericLazyInspectRecorder(capacity, parent_node));
  }

  NumericLazyInspectRecorder(size_t capacity, inspect::Node& parent_node)
      : LazyInspectRecorderBase<T>(capacity, parent_node) {}
};

}  // namespace power_observability::internal

#endif  // LIB_POWER_STATE_RECORDER_CPP_NUMERIC_STATE_RECORDER_INTERNAL_H_
