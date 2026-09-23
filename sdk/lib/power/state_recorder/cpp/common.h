// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_COMMON_H_
#define LIB_POWER_STATE_RECORDER_CPP_COMMON_H_

#include <lib/inspect/component/cpp/component.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <algorithm>
#include <mutex>
#include <string>
#include <vector>

namespace power_observability {

// Options that specify how data is recorded.
struct RecorderOptions {
  // The number of entries a recorder will retain before overwriting older entries.
  size_t capacity;

  // Whether to record Inspect data in a lazy node.
  //
  // If true, timestamp resolution will be limited to milliseconds to optimize memory usage to 4
  // bytes per timestamp instead of 8 bytes). If necessary, we could support toggling this
  // optimization (retaining nanosecond-resolution timestamps at the expense of more memory) in the
  // future.
  bool lazy_record;
};

// Manages state associated with all StateRecorder instances linked to a particular inspector.
//
// Only one StateRecorderManager should be created for a given ComponentInspector instance, as
// it corresponds to a specifically-named child of the inspector's root.
class StateRecorderManager final {
 public:
  explicit StateRecorderManager(inspect::ComponentInspector& inspector)
      : recorders_root_(inspector.root().CreateChild("power_observability_state_recorders")) {}

  zx::result<inspect::Node> RegisterName(std::string& name) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (std::ranges::find(names_in_use_, name) != names_in_use_.end()) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    names_in_use_.push_back(name);
    return zx::ok(recorders_root_.CreateChild(name));
  }

  void UnregisterName(std::string& name) {
    std::lock_guard<std::mutex> lock(mutex_);
    std::erase(names_in_use_, name);
  }

 private:
  // Represents a set, but implemented using a vector due to expected small number of elements.
  std::vector<std::string> names_in_use_ __TA_GUARDED(mutex_);
  inspect::Node recorders_root_;
  std::mutex mutex_;
};

}  // namespace power_observability

#endif  // LIB_POWER_STATE_RECORDER_CPP_COMMON_H_
