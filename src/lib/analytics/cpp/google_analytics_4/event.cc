// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/google_analytics_4/event.h"

namespace analytics::google_analytics_4 {

Event::~Event() = default;

Event::Event(std::string name) : name_(std::move(name)) {
  timestamp_micros_ = std::chrono::duration_cast<std::chrono::microseconds>(
      std::chrono::system_clock::now().time_since_epoch());
}

void Event::SetParameter(std::string name, Value value) {
  if (!parameters_opt_.has_value()) {
    parameters_opt_.emplace();
  }
  (*parameters_opt_)[std::move(name)] = std::move(value);
}

}  // namespace analytics::google_analytics_4
