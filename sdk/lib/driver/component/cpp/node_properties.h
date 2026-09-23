// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_NODE_PROPERTIES_H_
#define LIB_DRIVER_COMPONENT_CPP_NODE_PROPERTIES_H_

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <lib/driver/component/cpp/node_wire_properties.h>

#include <string_view>

namespace fdf {

inline fuchsia_driver_framework::NodeProperty2 MakeProperty2(std::string_view key,
                                                             std::string_view value) {
  return fuchsia_driver_framework::NodeProperty2{
      {.key = std::string(key),
       .value = fuchsia_driver_framework::NodePropertyValue::WithStringValue(std::string(value))}};
}

inline fuchsia_driver_framework::NodeProperty2 MakeProperty2(std::string_view key,
                                                             const char* value) {
  return MakeProperty2(key, std::string_view(value));
}

inline fuchsia_driver_framework::NodeProperty2 MakeProperty2(std::string_view key, bool value) {
  return fuchsia_driver_framework::NodeProperty2{
      {.key = std::string(key),
       .value = fuchsia_driver_framework::NodePropertyValue::WithBoolValue(value)}};
}

inline fuchsia_driver_framework::NodeProperty2 MakeProperty2(std::string_view key, uint32_t value) {
  return fuchsia_driver_framework::NodeProperty2{
      {.key = std::string(key),
       .value = fuchsia_driver_framework::NodePropertyValue::WithIntValue(value)}};
}

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_NODE_PROPERTIES_H_
