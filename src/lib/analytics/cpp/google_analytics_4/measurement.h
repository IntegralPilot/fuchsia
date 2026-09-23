// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_MEASUREMENT_H_
#define SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_MEASUREMENT_H_

#include <cstddef>
#include <memory>
#include <optional>
#include <string>
#include <vector>

#include "src/lib/analytics/cpp/google_analytics_4/event.h"

namespace analytics::google_analytics_4 {

inline constexpr size_t kMeasurementEventMaxCount = 25;

// Represents a JSON post body for GA4 measurement protocol as described in
// https://developers.google.com/analytics/devguides/collection/protocol/ga4/reference?client_type=gtag#payload_post_body
// We dropped a few optional JSON object keys (e.g. "non_personalized_ads") that we will not use.
class Measurement {
 public:
  // Getters for the corresponding JSON object keys
  const auto& client_id() const { return client_id_; }
  const auto& event_ptrs() const { return event_ptrs_; }
  const auto& user_properties_opt() const { return user_properties_opt_; }

  explicit Measurement(std::string client_id);

  // Add an Event to the measurement. One measurement can include up to 25 events.
  void AddEvent(std::unique_ptr<Event> event);
  // Set one user property.
  void SetUserProperty(std::string name, Value value);
  // Replace the whole user_properties map. Previously set values will be lost.
  void SetUserProperties(std::map<std::string, Value> user_properties);
  // Replace the whole events vector. Previously added events will be lost.
  void SetEvents(std::vector<std::unique_ptr<Event>> event_ptrs);

 private:
  std::string client_id_;
  std::vector<std::unique_ptr<Event>> event_ptrs_;
  std::optional<std::map<std::string, Value>> user_properties_opt_;
};

}  // namespace analytics::google_analytics_4

#endif  // SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_MEASUREMENT_H_
