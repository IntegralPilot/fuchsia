// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_COMPONENT_CPP_NODE_WIRE_PROPERTIES_H_
#define LIB_DRIVER_COMPONENT_CPP_NODE_WIRE_PROPERTIES_H_

#include <fidl/fuchsia.driver.framework/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/string_view.h>

#include <string_view>

namespace fdf {

inline fuchsia_driver_framework::wire::NodeProperty MakeProperty(fidl::AnyArena& arena,
                                                                 std::string_view key,
                                                                 std::string_view value) {
  return fuchsia_driver_framework::wire::NodeProperty{
      .key = fuchsia_driver_framework::wire::NodePropertyKey::WithStringValue(arena, key),
      .value = fuchsia_driver_framework::wire::NodePropertyValue::WithStringValue(arena, value)};
}

inline fuchsia_driver_framework::wire::NodeProperty MakeProperty(fidl::AnyArena& arena,
                                                                 std::string_view key,
                                                                 const char* value) {
  return MakeProperty(arena, key, std::string_view(value));
}

inline fuchsia_driver_framework::wire::NodeProperty MakeProperty(fidl::AnyArena& arena,
                                                                 std::string_view key, bool value) {
  return fuchsia_driver_framework::wire::NodeProperty{
      .key = fuchsia_driver_framework::wire::NodePropertyKey::WithStringValue(arena, key),
      .value = fuchsia_driver_framework::wire::NodePropertyValue::WithBoolValue(value)};
}

inline fuchsia_driver_framework::wire::NodeProperty MakeProperty(fidl::AnyArena& arena,
                                                                 std::string_view key,
                                                                 uint32_t value) {
  return fuchsia_driver_framework::wire::NodeProperty{
      .key = fuchsia_driver_framework::wire::NodePropertyKey::WithStringValue(arena, key),
      .value = fuchsia_driver_framework::wire::NodePropertyValue::WithIntValue(value)};
}

inline fuchsia_driver_framework::wire::NodeProperty2 MakeProperty2(fidl::AnyArena& arena,
                                                                   std::string_view key,
                                                                   std::string_view value) {
  return fuchsia_driver_framework::wire::NodeProperty2{
      .key = fidl::StringView(arena, key),
      .value = fuchsia_driver_framework::wire::NodePropertyValue::WithStringValue(arena, value)};
}

inline fuchsia_driver_framework::wire::NodeProperty2 MakeProperty2(fidl::AnyArena& arena,
                                                                   std::string_view key,
                                                                   const char* value) {
  return MakeProperty2(arena, key, std::string_view(value));
}

inline fuchsia_driver_framework::wire::NodeProperty2 MakeProperty2(fidl::AnyArena& arena,
                                                                   std::string_view key,
                                                                   bool value) {
  return fuchsia_driver_framework::wire::NodeProperty2{
      .key = fidl::StringView(arena, key),
      .value = fuchsia_driver_framework::wire::NodePropertyValue::WithBoolValue(value)};
}

inline fuchsia_driver_framework::wire::NodeProperty2 MakeProperty2(fidl::AnyArena& arena,
                                                                   std::string_view key,
                                                                   uint32_t value) {
  return fuchsia_driver_framework::wire::NodeProperty2{
      .key = fidl::StringView(arena, key),
      .value = fuchsia_driver_framework::wire::NodePropertyValue::WithIntValue(value)};
}

}  // namespace fdf

#endif  // LIB_DRIVER_COMPONENT_CPP_NODE_WIRE_PROPERTIES_H_
