// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_MISC_DRIVERS_COMPAT_COMPOSITE_NODE_SPEC_UTIL_H_
#define SRC_DEVICES_MISC_DRIVERS_COMPAT_COMPOSITE_NODE_SPEC_UTIL_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/ddk/device.h>

#include <string_view>

#include <bind/fuchsia/cpp/bind.h>

inline zx::result<fuchsia_driver_framework::wire::BindRule> ConvertBindRuleToFidl(
    fidl::AnyArena& allocator, const bind_rule_t& bind_rule) {
  fuchsia_driver_framework::wire::NodePropertyKey property_key;

  switch (bind_rule.key.key_type) {
    case DEVICE_BIND_PROPERTY_KEY_INT: {
      property_key =
          fuchsia_driver_framework::wire::NodePropertyKey::WithIntValue(bind_rule.key.data.int_key);
      break;
    }
    case DEVICE_BIND_PROPERTY_KEY_STRING: {
      if (bind_rule.key.data.str_key == nullptr) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      property_key = fuchsia_driver_framework::wire::NodePropertyKey::WithStringValue(
          allocator, allocator, bind_rule.key.data.str_key);
      break;
    }
    default: {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  if (bind_rule.values_count > 0 && bind_rule.values == nullptr) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto bind_rule_values = fidl::VectorView<fuchsia_driver_framework::wire::NodePropertyValue>(
      allocator, bind_rule.values_count);
  for (size_t i = 0; i < bind_rule.values_count; i++) {
    const auto& bind_rule_val = bind_rule.values[i];
    switch (bind_rule_val.data_type) {
      case ZX_DEVICE_PROPERTY_VALUE_INT: {
        bind_rule_values[i] = fuchsia_driver_framework::wire::NodePropertyValue::WithIntValue(
            bind_rule_val.data.int_value);
        break;
      }
      case ZX_DEVICE_PROPERTY_VALUE_STRING: {
        if (bind_rule_val.data.str_value == nullptr) {
          return zx::error(ZX_ERR_INVALID_ARGS);
        }
        auto str_val =
            fidl::ObjectView<fidl::StringView>(allocator, allocator, bind_rule_val.data.str_value);
        bind_rule_values[i] =
            fuchsia_driver_framework::wire::NodePropertyValue::WithStringValue(str_val);
        break;
      }
      case ZX_DEVICE_PROPERTY_VALUE_BOOL: {
        bind_rule_values[i] = fuchsia_driver_framework::wire::NodePropertyValue::WithBoolValue(
            bind_rule_val.data.bool_value);
        break;
      }
      case ZX_DEVICE_PROPERTY_VALUE_ENUM: {
        if (bind_rule_val.data.enum_value == nullptr) {
          return zx::error(ZX_ERR_INVALID_ARGS);
        }
        auto enum_val =
            fidl::ObjectView<fidl::StringView>(allocator, allocator, bind_rule_val.data.enum_value);
        bind_rule_values[i] =
            fuchsia_driver_framework::wire::NodePropertyValue::WithEnumValue(enum_val);
        break;
      }
      default: {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    }
  }

  fuchsia_driver_framework::wire::Condition condition;
  switch (bind_rule.condition) {
    case DEVICE_BIND_RULE_CONDITION_ACCEPT: {
      condition = fuchsia_driver_framework::wire::Condition::kAccept;
      break;
    }
    case DEVICE_BIND_RULE_CONDITION_REJECT: {
      condition = fuchsia_driver_framework::wire::Condition::kReject;
      break;
    }
    default: {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  return zx::ok(fuchsia_driver_framework::wire::BindRule{
      .key = property_key,
      .condition = condition,
      .values = bind_rule_values,
  });
}

inline zx::result<fuchsia_driver_framework::wire::NodeProperty> ConvertBindPropToFidl(
    fidl::AnyArena& allocator, const device_bind_prop_t& bind_prop) {
  auto node_property = fuchsia_driver_framework::wire::NodeProperty{};

  switch (bind_prop.key.key_type) {
    case DEVICE_BIND_PROPERTY_KEY_INT: {
      node_property.key =
          fuchsia_driver_framework::wire::NodePropertyKey::WithIntValue(bind_prop.key.data.int_key);
      break;
    }
    case DEVICE_BIND_PROPERTY_KEY_STRING: {
      if (bind_prop.key.data.str_key == nullptr) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      node_property.key = fuchsia_driver_framework::wire::NodePropertyKey::WithStringValue(
          allocator, allocator, bind_prop.key.data.str_key);
      break;
    }
    default: {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  switch (bind_prop.value.data_type) {
    case ZX_DEVICE_PROPERTY_VALUE_INT: {
      node_property.value = fuchsia_driver_framework::wire::NodePropertyValue::WithIntValue(
          bind_prop.value.data.int_value);
      break;
    }
    case ZX_DEVICE_PROPERTY_VALUE_STRING: {
      if (bind_prop.value.data.str_value == nullptr) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      node_property.value = fuchsia_driver_framework::wire::NodePropertyValue::WithStringValue(
          allocator, allocator, bind_prop.value.data.str_value);
      break;
    }
    case ZX_DEVICE_PROPERTY_VALUE_BOOL: {
      node_property.value = fuchsia_driver_framework::wire::NodePropertyValue::WithBoolValue(
          bind_prop.value.data.bool_value);
      break;
    }
    case ZX_DEVICE_PROPERTY_VALUE_ENUM: {
      if (bind_prop.value.data.enum_value == nullptr) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      node_property.value = fuchsia_driver_framework::wire::NodePropertyValue::WithEnumValue(
          fidl::ObjectView<fidl::StringView>(allocator, allocator,
                                             bind_prop.value.data.enum_value));
      break;
    }
    default: {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  return zx::ok(node_property);
}

inline zx::result<fuchsia_driver_framework::wire::ParentSpec> ConvertNodeRepresentation(
    fidl::AnyArena& allocator, const parent_spec_t& node) {
  if (node.bind_rule_count > 0 && node.bind_rules == nullptr) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (node.property_count > 0 && node.properties == nullptr) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fidl::VectorView<fuchsia_driver_framework::wire::BindRule> bind_rules(allocator,
                                                                        node.bind_rule_count);
  for (size_t i = 0; i < node.bind_rule_count; i++) {
    auto bind_rule_result = ConvertBindRuleToFidl(allocator, node.bind_rules[i]);
    if (!bind_rule_result.is_ok()) {
      return bind_rule_result.take_error();
    }

    bind_rules[i] = std::move(bind_rule_result.value());
  }

  bool has_service_property = false;
  std::string_view service_name;
  for (size_t i = 0; i < node.property_count; i++) {
    if (node.properties[i].key.key_type == DEVICE_BIND_PROPERTY_KEY_STRING &&
        node.properties[i].key.data.str_key != nullptr) {
      std::string_view key_str(node.properties[i].key.data.str_key);
      if (key_str == "fuchsia.Service" || key_str == bind_fuchsia::SERVICE) {
        has_service_property = true;
      } else if (key_str.ends_with(".Service") || key_str.ends_with(".PathService") ||
                 key_str.ends_with(".TargetService") || key_str.ends_with(".SubTargetService") ||
                 key_str.ends_with(".PinStatesService")) {
        service_name = key_str;
      }
    }
  }

  const bool add_service_prop = !has_service_property && !service_name.empty();
  const size_t prop_count = node.property_count + (add_service_prop ? 1 : 0);
  fidl::VectorView<fuchsia_driver_framework::wire::NodeProperty> props(allocator, prop_count);
  for (size_t i = 0; i < node.property_count; i++) {
    auto prop_result = ConvertBindPropToFidl(allocator, node.properties[i]);
    if (!prop_result.is_ok()) {
      return prop_result.take_error();
    }
    props[i] = std::move(prop_result.value());
  }

  if (add_service_prop) {
    props[node.property_count] = fuchsia_driver_framework::wire::NodeProperty{
        .key = fuchsia_driver_framework::wire::NodePropertyKey::WithStringValue(
            allocator, allocator, bind_fuchsia::SERVICE),
        .value = fuchsia_driver_framework::wire::NodePropertyValue::WithStringValue(
            allocator, allocator, service_name),
    };
  }

  return zx::ok(fuchsia_driver_framework::wire::ParentSpec{
      .bind_rules = bind_rules,
      .properties = props,
  });
}

#endif  // SRC_DEVICES_MISC_DRIVERS_COMPAT_COMPOSITE_NODE_SPEC_UTIL_H_
