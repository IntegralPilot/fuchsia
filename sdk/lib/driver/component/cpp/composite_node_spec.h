// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_COMPOSITE_NODE_SPEC_H_
#define LIB_DRIVER_COMPONENT_CPP_COMPOSITE_NODE_SPEC_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>

#include <string_view>

namespace fdf {

// String keys with string values
inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    cpp20::span<const std::string_view> values) {
  std::vector<fuchsia_driver_framework::NodePropertyValue> values_vec;
  values_vec.reserve(values.size());
  for (auto val : values) {
    values_vec.push_back(
        fuchsia_driver_framework::NodePropertyValue::WithStringValue(std::string(val)));
  }

  return fuchsia_driver_framework::BindRule2(std::string(key), condition, values_vec);
}

inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    const std::string_view value) {
  return MakeBindRule(key, condition, cpp20::span<const std::string_view>{{value}});
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              const std::string_view value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, value);
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(
    const std::string_view key, cpp20::span<const std::string_view> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, values);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              const std::string_view value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, value);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(
    const std::string_view key, cpp20::span<const std::string_view> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, values);
}

// String keys with char* values
inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    cpp20::span<const char*> values) {
  std::vector<std::string_view> vec;
  vec.reserve(values.size());
  for (auto val : values) {
    vec.push_back(val);
  }
  return MakeBindRule(key, condition, vec);
}

inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    const char* value) {
  return MakeBindRule(key, condition, std::string_view(value));
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              const char* value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, value);
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              cpp20::span<const char*> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, values);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              const char* value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, value);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              cpp20::span<const char*> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, values);
}

// String keys with bool values
inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    cpp20::span<const bool> values) {
  std::vector<fuchsia_driver_framework::NodePropertyValue> values_vec;
  values_vec.reserve(values.size());
  for (auto val : values) {
    values_vec.push_back(fuchsia_driver_framework::NodePropertyValue::WithBoolValue(val));
  }

  return fuchsia_driver_framework::BindRule2(std::string(key), condition, values_vec);
}

inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    const bool value) {
  return MakeBindRule(key, condition, {{value}});
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              const bool value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, value);
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              cpp20::span<const bool> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, values);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              const bool value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, value);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              cpp20::span<const bool> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, values);
}

// String keys with int values
inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    cpp20::span<const uint32_t> values) {
  std::vector<fuchsia_driver_framework::NodePropertyValue> values_vec;
  values_vec.reserve(values.size());
  for (auto val : values) {
    values_vec.push_back(fuchsia_driver_framework::NodePropertyValue::WithIntValue(val));
  }

  return fuchsia_driver_framework::BindRule2(std::string(key), condition, values_vec);
}

inline fuchsia_driver_framework::BindRule2 MakeBindRule(
    const std::string_view key, const fuchsia_driver_framework::Condition condition,
    const uint32_t value) {
  return MakeBindRule(key, condition, cpp20::span<const uint32_t>{{value}});
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              const uint32_t value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, value);
}

inline fuchsia_driver_framework::BindRule2 MakeAcceptBindRule(const std::string_view key,
                                                              cpp20::span<const uint32_t> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kAccept, values);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              const uint32_t value) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, value);
}

inline fuchsia_driver_framework::BindRule2 MakeRejectBindRule(const std::string_view key,
                                                              cpp20::span<const uint32_t> values) {
  return MakeBindRule(key, fuchsia_driver_framework::Condition::kReject, values);
}

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_COMPOSITE_NODE_SPEC_H_
