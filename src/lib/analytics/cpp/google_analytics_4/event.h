// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_EVENT_H_
#define SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_EVENT_H_

#include <chrono>
#include <map>
#include <optional>
#include <string>
#include <variant>

namespace analytics::google_analytics_4 {

using Value = std::variant<std::string, int64_t, double, bool>;

// Represents an GA4 event as described in
// https://developers.google.com/analytics/devguides/collection/protocol/ga4/reference?client_type=gtag#payload_post_body
class Event {
 public:
  // Event is an abstract class
  virtual ~Event() = 0;

  // Getters for the corresponding JSON object keys
  const auto& name() const { return name_; }
  const auto& parameters_opt() const { return parameters_opt_; }
  const auto& timestamp_micros() const { return timestamp_micros_; }

 protected:
  explicit Event(std::string name);
  void SetParameter(std::string name, Value value);

 private:
  std::string name_;
  std::optional<std::map<std::string, Value>> parameters_opt_;
  // In the spec, timestamp_micros is also optional. But we decided to
  // include a timestamp generated at construction for every Event.
  std::chrono::microseconds timestamp_micros_;
};

}  // namespace analytics::google_analytics_4

#endif  // SRC_LIB_ANALYTICS_CPP_GOOGLE_ANALYTICS_4_EVENT_H_
