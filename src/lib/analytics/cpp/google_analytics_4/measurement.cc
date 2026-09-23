// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/analytics/cpp/google_analytics_4/measurement.h"

namespace analytics::google_analytics_4 {

Measurement::Measurement(std::string client_id) : client_id_(std::move(client_id)) {}

void Measurement::AddEvent(std::unique_ptr<Event> event) {
  event_ptrs_.push_back(std::move(event));
}

void Measurement::SetUserProperty(std::string name, Value value) {
  if (!user_properties_opt_.has_value()) {
    user_properties_opt_.emplace();
  }
  (*user_properties_opt_)[std::move(name)] = std::move(value);
}

void Measurement::SetUserProperties(std::map<std::string, Value> user_properties) {
  user_properties_opt_ = std::move(user_properties);
}

void Measurement::SetEvents(std::vector<std::unique_ptr<Event>> event_ptrs) {
  event_ptrs_ = std::move(event_ptrs);
}

}  // namespace analytics::google_analytics_4
