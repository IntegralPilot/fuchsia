// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_PROPERTY_CONVERSION_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_PROPERTY_CONVERSION_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>

namespace driver_manager {

inline fuchsia_driver_framework::NodeProperty2 ToProperty2(
    const fuchsia_driver_framework::NodeProperty& property) {
  ZX_ASSERT(property.key().Which() == fuchsia_driver_framework::NodePropertyKey::Tag::kStringValue);
  return fuchsia_driver_framework::NodeProperty2{
      {.key = property.key().string_value().value(), .value = property.value()}};
}

inline fuchsia_driver_framework::NodeProperty2 ToProperty2(
    const fuchsia_driver_framework::wire::NodeProperty property) {
  ZX_ASSERT(property.key.is_string_value());
  return fuchsia_driver_framework::NodeProperty2{
      {.key = std::string(property.key.string_value().get()),
       .value = fidl::ToNatural(property.value)}};
}

inline fuchsia_driver_framework::NodeProperty ToDeprecatedProperty(
    const fuchsia_driver_framework::NodeProperty2& property) {
  return fuchsia_driver_framework::NodeProperty{
      {.key =
           fuchsia_driver_framework::NodePropertyKey::WithStringValue(std::string(property.key())),
       .value = property.value()}};
}

inline fuchsia_driver_framework::wire::NodeProperty ToDeprecatedProperty(
    fidl::AnyArena& allocator, const fuchsia_driver_framework::NodeProperty2& property) {
  fuchsia_driver_framework::wire::NodePropertyValue value;
  return fuchsia_driver_framework::wire::NodeProperty{
      .key = fidl::ToWire(
          allocator, fuchsia_driver_framework::NodePropertyKey::WithStringValue(property.key())),
      .value = fidl::ToWire(allocator, property.value())};
}

inline fuchsia_driver_framework::wire::NodeProperty ToDeprecatedProperty(
    fidl::AnyArena& allocator, const fuchsia_driver_framework::wire::NodeProperty2 property) {
  fuchsia_driver_framework::wire::NodePropertyValue value;
  return fuchsia_driver_framework::wire::NodeProperty{
      .key = fidl::ToWire(allocator, fuchsia_driver_framework::NodePropertyKey::WithStringValue(
                                         std::string(property.key.get()))),
      .value = fidl::ToWire(allocator, fidl::ToNatural(property.value))};
}

inline fuchsia_driver_framework::BindRule2 ToBindRule2(
    const fuchsia_driver_framework::BindRule& bind_rule) {
  ZX_ASSERT(bind_rule.key().Which() ==
            fuchsia_driver_framework::NodePropertyKey::Tag::kStringValue);
  return fuchsia_driver_framework::BindRule2{{.key = bind_rule.key().string_value().value(),
                                              .condition = bind_rule.condition(),
                                              .values = bind_rule.values()}};
}

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_NODE_PROPERTY_CONVERSION_H_
